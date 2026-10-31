//! The setup pass's and the lend's way into a container on devpod's local docker
//! provider: `docker exec`, with `devpod ssh` kept as the way in for everything else.
//!
//! # Why
//!
//! The pass and the lend are two of a cold launch's trips, and on this host
//! `devpod ssh --command true` costs 2.1s to 2.6s where `docker exec` into the same
//! container costs about 0.14s. devpod spends the difference on its own side:
//! loading the provider, starting the credentials tunnel and an ssh handshake over
//! `docker exec`. The tar the lend streams pays for that handshake too: 258MB went
//! through `devpod ssh` in 10.5s and through `docker exec -i` in 1.6s.
//!
//! devlaunch#424 asked whether these two trips should move onto dl's own OpenSSH
//! transport instead. Measured against the same container: the first OpenSSH trip
//! through devpod's `ProxyCommand` costs 3.6s (it is a `devpod ssh --stdio` plus a
//! second handshake), and only the trips after it are 0.02s. A cold pass has no
//! master to join, so OpenSSH would save one trip of two at best, where this saves
//! both and the tar time. That is the evidence this route was chosen on.
//!
//! # The session this has to reproduce
//!
//! The pass decides what to lend from what its login shell finds on `PATH`, so a
//! route that saw a different environment would lend into a container that did not
//! need it, or skip one that did. devpod 0.26's session, read from its source and
//! measured against the real one, is:
//!
//! 1. As root, `su -c '<devpod helper ssh-server>' <user>` for any user but root.
//!    `su` is what sets `HOME`, `USER`, `LOGNAME`, `SHELL`, the PAM environment and
//!    the umask (0002 under Ubuntu's `USERGROUPS_ENAB`, where `docker exec` gives
//!    0022). A root session skips `su`.
//! 2. That helper applies `/etc/envfile.json` over its environment: devpod's merge
//!    of `remoteEnv` and every `--workspace-env` it was given, `PATH` and the
//!    forwarded `GH_TOKEN` among them.
//! 3. It changes to the workspace folder (`MergedConfig.workspaceFolder`, else
//!    `/workspaces/<id>`), or to the home directory when that does not exist.
//! 4. It runs `<login shell> -c <command>`.
//!
//! [`spec`] does the same four things in the same order. The envfile is read in a
//! trip of its own and parsed here, on the host, because the container is not
//! promised a JSON parser. Its exports travel to the container in an environment
//! variable rather than in argv, for the reason [`crate::clients::ssh`] sends
//! variables by name: a token in argv is a token `ps` shows to every user.
//!
//! What still differs, measured with `env | sort` under both routes: devpod
//! forwards the host's ssh-agent into the session and this route does not, so
//! `SSH_AUTH_SOCK` is the container's own. Nothing the pass or the lend runs talks
//! to an ssh-agent.
//!
//! # When it is not taken
//!
//! Every doubt falls back to `devpod ssh`, never to a guess. [`target`] declines a
//! provider other than `docker`, a provider pointed at a daemon of its own
//! (`DOCKER_HOST`) or at a docker binary of its own (`DOCKER_PATH`), a workspace
//! on a git subpath, a result with no container id, and a remote user that is not
//! a plain lowercase name. The envfile read is also the proof that the container
//! is reachable: it runs as the first `docker exec`, and a container that is gone,
//! on another daemon (a rootless or remote docker context that devpod's provider
//! does not share) or not running answers without the mark, so nothing is taken.
//!
//! A trip that does not arrive is a trip `devpod ssh` makes instead. Arrival is
//! the mark the prelude prints on stderr just before it hands over to the
//! command, so a `docker exec` or `su` that failed is told apart from a command
//! that ran and exited non-zero, which for the lend is the arch/libc gate and must
//! keep meaning "rejected".

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::clients::devpod_home::{DevpodHome, sole_workspace_records};
use crate::runner::{CapturedText, EnvSpec, Exit, Invocation, Outcome, Runner, SpawnSpec};
use crate::shell::quote;
use crate::timing;

/// The program every trip here runs.
const DOCKER: &str = crate::clients::docker::PROGRAM;

/// The one devpod provider this route reaches into.
const PROVIDER: &str = "docker";

/// devpod's own record of the remote user, beside `remoteUser`
/// (`pkg/devcontainer/config/result.go`'s `UserLabel`).
const USER_LABEL: &str = "devpod.user";

/// Where devpod keeps the environment its ssh helper applies.
const ENVFILE: &str = "/etc/envfile.json";

/// The first line of a successful envfile read, so a read is told apart from a
/// `docker exec` that never got in.
const ENVFILE_MARK: &str = "devlaunch-envfile";

/// What the prelude prints on stderr once it is inside, as the session it built.
pub(crate) const ARRIVED_MARK: &str = "devlaunch-direct-arrived";

/// How long the envfile read may take before the route is given up. A read of
/// one small file; a daemon that does not answer in this is one to go round.
const READ_ENVFILE: Duration = Duration::from_secs(15);

/// The variables the prelude reads, and unsets before anything else sees them.
const ENV_EXPORTS: &str = "DEVLAUNCH_DIRECT_ENV";
const ENV_WORKDIR: &str = "DEVLAUNCH_DIRECT_WORKDIR";
const ENV_COMMAND: &str = "DEVLAUNCH_DIRECT_COMMAND";

/// The script that turns a `docker exec` into devpod's session, steps 2 to 4.
///
/// One line, with no single quote, backslash, `!` or newline in it, because for a
/// named user it is the argument of `su -c`, and `su` hands that to the user's own
/// login shell to parse: inside single quotes that is literal text to `sh`, bash,
/// zsh, fish and csh alike. Positional parameters rather than shell variables, so
/// nothing the envfile exports can overwrite the command or the folder before
/// they are used.
const PRELUDE: &str = concat!(
    "set -- \"$DEVLAUNCH_DIRECT_COMMAND\" \"$DEVLAUNCH_DIRECT_WORKDIR\"; ",
    "unset DEVLAUNCH_DIRECT_COMMAND DEVLAUNCH_DIRECT_WORKDIR; ",
    "eval \"$DEVLAUNCH_DIRECT_ENV\"; unset DEVLAUNCH_DIRECT_ENV; ",
    "cd \"$2\" 2>/dev/null || cd; unset OLDPWD; ",
    "echo devlaunch-direct-arrived >&2; ",
    "exec \"${SHELL:-/bin/sh}\" -c \"$1\"",
);

/// Who the session runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
enum User {
    /// devpod runs a root session without `su`, so this route does too.
    Root,
    /// Anybody else, through `su` as devpod does.
    Named(String),
}

/// What the workspace's records say about reaching its container.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    container: String,
    user: User,
    workdir: String,
}

/// A container this route can reach, with the environment devpod would apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Direct {
    target: Target,
    /// `export NAME=<quoted value>` lines, one per envfile entry.
    exports: String,
}

/// Why the records say to take `devpod ssh`. Each arm is one fallback rule, and
/// the tests below pin each one.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Decline {
    /// The provider is not devpod's local `docker` one.
    NotDockerProvider,
    /// The provider names a daemon or a docker binary of its own.
    OwnDaemon,
    /// devpod resolves the folder of a git-subpath workspace by another rule.
    SubPath,
    /// No container id, or one that is not a docker id.
    NoContainer,
    /// A user name devpod would lowercase, or one that could be read as an option.
    UnusualUser,
    /// A folder `cd` cannot be handed.
    UnusableWorkdir,
    /// The envfile read did not get in, or what it read is not devpod's format.
    Unreadable,
}

/// The route for this workspace, or `None` for `devpod ssh`.
///
/// One `docker exec` when the records allow it, and none when they do not.
pub(crate) fn resolve(
    runner: &dyn Runner,
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Option<Direct> {
    let (record, result) = sole_workspace_records(devpod_home, workspace_id)?;
    let record = read_json(&record)?;
    let result = read_json(&result)?;
    let target = target(&record, &result, workspace_id).ok()?;
    let exports = read_envfile(runner, &target.container).ok()?;
    Some(Direct { target, exports })
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The container, user and folder `devpod ssh` would use, from devpod's own
/// `workspace.json` (`record`) and `workspace_result.json` (`result`).
fn target(record: &Value, result: &Value, workspace_id: &str) -> Result<Target, Decline> {
    let provider = &record["provider"];
    if provider["name"].as_str() != Some(PROVIDER) {
        return Err(Decline::NotDockerProvider);
    }
    let option = |name: &str| provider["options"][name]["value"].as_str().unwrap_or("");
    if !option("DOCKER_HOST").is_empty() || !matches!(option("DOCKER_PATH"), "" | "docker") {
        return Err(Decline::OwnDaemon);
    }
    if !record["source"]["gitSubPath"]
        .as_str()
        .unwrap_or("")
        .is_empty()
    {
        return Err(Decline::SubPath);
    }

    let container = result["ContainerDetails"]["ID"].as_str().unwrap_or("");
    if container.is_empty() || !container.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Decline::NoContainer);
    }

    let user = remote_user(result);
    let user = if user == "root" {
        User::Root
    } else if is_plain_user(user) {
        User::Named(user.to_owned())
    } else {
        return Err(Decline::UnusualUser);
    };

    let workdir = match result["MergedConfig"]["workspaceFolder"].as_str() {
        Some(folder) if !folder.is_empty() => folder.to_owned(),
        _ => format!("/workspaces/{workspace_id}"),
    };
    if workdir.contains('\0') {
        return Err(Decline::UnusableWorkdir);
    }

    Ok(Target {
        container: container.to_owned(),
        user,
        workdir,
    })
}

/// devpod's `GetRemoteUser`: `remoteUser`, else the `devpod.user` label, else
/// `containerUser`, else root. `devpod up` writes this into the ssh alias's `User`
/// line, which is where `devpod ssh` reads it back.
fn remote_user(result: &Value) -> &str {
    let merged = &result["MergedConfig"];
    [
        &merged["remoteUser"],
        &result["ContainerDetails"]["Config"]["Labels"][USER_LABEL],
        &merged["containerUser"],
    ]
    .into_iter()
    .filter_map(Value::as_str)
    .find(|user| !user.is_empty())
    .unwrap_or("root")
}

/// A name devpod reads back unchanged and `su` cannot take for an option.
///
/// Lowercase only, because devpod lowercases the ssh alias's `User` line when it
/// reads it back, so a `Vscode` here would be a different user from devpod's.
fn is_plain_user(user: &str) -> bool {
    let mut bytes = user.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == b'_')
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
}

/// Read devpod's envfile out of the container, as root, the way its helper reads
/// it before it drops to the user.
fn read_envfile(runner: &dyn Runner, container: &str) -> Result<String, Decline> {
    let script = format!("echo {ENVFILE_MARK}; cat {ENVFILE} 2>/dev/null; true");
    let spec = SpawnSpec::from(
        Invocation::new(DOCKER).with_args(["exec", "-u", "0", container, "/bin/sh", "-c", &script]),
    )
    .with_stdin_null()
    .with_timeout(READ_ENVFILE);
    let _span = timing::span("docker exec");
    match runner.capture(&spec) {
        Outcome::Ran { exit, io } if exit.is_success() => exports(&io.stdout),
        _ => Err(Decline::Unreadable),
    }
}

/// The envfile read, as `export` lines for the prelude to `eval`.
///
/// No file is no exports, which is what devpod's helper applies then too. A file
/// that is there and is not a JSON object of strings under `env`, or that names a
/// variable a shell cannot export, is a doubt.
fn exports(read: &str) -> Result<String, Decline> {
    let body = read
        .strip_prefix(ENVFILE_MARK)
        .and_then(|rest| rest.strip_prefix('\n'))
        .ok_or(Decline::Unreadable)?;
    if body.trim().is_empty() {
        return Ok(String::new());
    }
    let document: Value = serde_json::from_str(body).map_err(|_| Decline::Unreadable)?;
    let env = match &document["env"] {
        Value::Null => return Ok(String::new()),
        Value::Object(env) => env,
        _ => return Err(Decline::Unreadable),
    };
    let mut lines = BTreeMap::new();
    for (name, value) in env {
        let value = value.as_str().ok_or(Decline::Unreadable)?;
        if !is_env_name(name) || value.contains('\0') {
            return Err(Decline::Unreadable);
        }
        lines.insert(name.as_str(), format!("export {name}={}", quote(value)));
    }
    Ok(lines.into_values().collect::<Vec<_>>().join("\n"))
}

fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The `docker exec` that runs `command` in devpod's session.
fn spec(direct: &Direct, command: &str) -> SpawnSpec {
    let target = &direct.target;
    let mut args: Vec<String> = [
        "exec",
        "-i",
        "-u",
        "0",
        "-e",
        ENV_EXPORTS,
        "-e",
        ENV_WORKDIR,
        "-e",
        ENV_COMMAND,
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(target.container.clone());
    match &target.user {
        User::Root => args.extend(["/bin/sh".to_owned(), "-c".to_owned(), PRELUDE.to_owned()]),
        User::Named(user) => args.extend([
            "su".to_owned(),
            "-c".to_owned(),
            format!("/bin/sh -c '{PRELUDE}'"),
            user.clone(),
        ]),
    }
    let env = EnvSpec::inherited()
        .and(ENV_EXPORTS, direct.exports.clone())
        .and(ENV_WORKDIR, target.workdir.clone())
        .and(ENV_COMMAND, command);
    SpawnSpec::new(Invocation::new(DOCKER).with_args(args).with_env(env))
}

/// Whether the prelude got as far as handing over to the command.
fn arrived(stderr: &str) -> bool {
    stderr.lines().any(|line| line == ARRIVED_MARK)
}

/// Run `command` and read both its streams, or `None` when the trip did not get in
/// and `devpod ssh` should make it instead.
///
/// The arrival mark is taken out of stderr, so what comes back reads as the
/// command's own.
pub(crate) fn capture(
    runner: &dyn Runner,
    direct: &Direct,
    command: &str,
) -> Option<(Exit, CapturedText)> {
    let spec = spec(direct, command).with_stdin_null();
    let _span = timing::span("docker exec");
    let Outcome::Ran { exit, io } = runner.capture(&spec) else {
        return None;
    };
    if !arrived(&io.stderr) {
        return None;
    }
    let stderr = io
        .stderr
        .lines()
        .filter(|line| *line != ARRIVED_MARK)
        .map(|line| format!("{line}\n"))
        .collect();
    Some((
        exit,
        CapturedText {
            stdout: io.stdout,
            stderr,
        },
    ))
}

/// Run `command` with `stdin` as its input and stderr on this process's, or `None`
/// when the trip did not get in and `devpod ssh` should make it instead.
///
/// The file is not consumed by a trip that did not get in, so the fallback sends
/// it again from the start: why the lend writes its tar to a file and not a pipe.
pub(crate) fn run_with_stdin(
    runner: &dyn Runner,
    direct: &Direct,
    command: &str,
    stdin: &Path,
) -> Option<Exit> {
    let spec = spec(direct, command).with_stdin_file(stdin);
    let _span = timing::span("docker exec");
    let mut got_in = false;
    let outcome = runner.session(&spec, &mut |line| {
        if line == ARRIVED_MARK {
            got_in = true;
        } else {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        }
    });
    match outcome {
        Outcome::Ran { exit, .. } if got_in => Some(exit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "716bcb8e2085d5c3512e78c12320fdacfdb0e58db69a429880faea18d48bb686";

    /// A `workspace.json` in the shape devpod 0.26 writes for the docker provider.
    fn record() -> Value {
        json!({
            "id": "ws",
            "uid": "default-ws-12345",
            "provider": {
                "name": "docker",
                "options": { "DOCKER_PATH": { "value": "docker" }, "INACTIVITY_TIMEOUT": {} }
            },
            "source": { "localFolder": "/src/ws" },
            "context": "default"
        })
    }

    /// A `workspace_result.json` in the shape devpod 0.26 writes.
    fn result() -> Value {
        json!({
            "MergedConfig": { "remoteUser": "vscode" },
            "ContainerDetails": { "ID": ID, "Config": { "Labels": { "devpod.user": "vscode" } } },
            "SubstitutionContext": { "ContainerWorkspaceFolder": "/workspaces/ws" }
        })
    }

    fn with(mut value: Value, pointer: &str, replacement: Value) -> Value {
        *value.pointer_mut(pointer).expect("a field the fixture has") = replacement;
        value
    }

    #[test]
    fn a_local_docker_workspace_is_reached_as_its_remote_user_in_its_folder() {
        assert_eq!(
            target(&record(), &result(), "ws"),
            Ok(Target {
                container: ID.to_owned(),
                user: User::Named("vscode".to_owned()),
                workdir: "/workspaces/ws".to_owned(),
            })
        );
    }

    #[test]
    fn the_folder_is_the_merged_workspace_folder_when_the_config_names_one() {
        let result = with(
            result(),
            "/MergedConfig",
            json!({ "remoteUser": "vscode", "workspaceFolder": "/src/elsewhere" }),
        );
        assert_eq!(
            target(&record(), &result, "ws").map(|target| target.workdir),
            Ok("/src/elsewhere".to_owned())
        );
    }

    #[test]
    fn every_provider_but_local_docker_takes_devpod_ssh() {
        for name in ["kubernetes", "ssh", "aws", ""] {
            let record = with(record(), "/provider/name", json!(name));
            assert_eq!(
                target(&record, &result(), "ws"),
                Err(Decline::NotDockerProvider),
                "{name}"
            );
        }
        let no_provider = with(record(), "/provider", json!(null));
        assert_eq!(
            target(&no_provider, &result(), "ws"),
            Err(Decline::NotDockerProvider)
        );
    }

    /// A provider pointed at a daemon of its own is one dl's `docker` may not reach.
    #[test]
    fn a_provider_with_its_own_daemon_or_binary_takes_devpod_ssh() {
        let remote = with(
            record(),
            "/provider/options",
            json!({ "DOCKER_HOST": { "value": "ssh://build-box" } }),
        );
        assert_eq!(target(&remote, &result(), "ws"), Err(Decline::OwnDaemon));
        let podman = with(
            record(),
            "/provider/options",
            json!({ "DOCKER_PATH": { "value": "podman" } }),
        );
        assert_eq!(target(&podman, &result(), "ws"), Err(Decline::OwnDaemon));
        let empty_host = with(
            record(),
            "/provider/options",
            json!({ "DOCKER_HOST": { "value": "" } }),
        );
        assert!(target(&empty_host, &result(), "ws").is_ok());
    }

    #[test]
    fn a_git_subpath_workspace_takes_devpod_ssh() {
        let record = with(
            record(),
            "/source",
            json!({ "gitRepository": "https://x/y", "gitSubPath": "sub" }),
        );
        assert_eq!(target(&record, &result(), "ws"), Err(Decline::SubPath));
    }

    #[test]
    fn a_result_without_a_docker_container_id_takes_devpod_ssh() {
        for id in [json!(null), json!(""), json!("abc;rm -rf /"), json!(7)] {
            let result = with(result(), "/ContainerDetails/ID", id.clone());
            assert_eq!(
                target(&record(), &result, "ws"),
                Err(Decline::NoContainer),
                "{id}"
            );
        }
        // The spelling the test fakes use is not devpod's, and reads as no id.
        let fake = with(result(), "/ContainerDetails", json!({ "Id": ID }));
        assert_eq!(target(&record(), &fake, "ws"), Err(Decline::NoContainer));
        assert_eq!(
            target(&record(), &json!({}), "ws"),
            Err(Decline::NoContainer)
        );
    }

    /// devpod's `GetRemoteUser` order, root last.
    #[test]
    fn the_user_is_the_one_devpod_ssh_would_be() {
        let user = |result: Value| target(&record(), &result, "ws").map(|target| target.user);
        let labelled = with(result(), "/MergedConfig", json!({}));
        assert_eq!(user(labelled), Ok(User::Named("vscode".to_owned())));
        let container_user = with(
            with(result(), "/MergedConfig", json!({ "containerUser": "dev" })),
            "/ContainerDetails/Config",
            json!({}),
        );
        assert_eq!(user(container_user), Ok(User::Named("dev".to_owned())));
        let nobody = with(
            with(result(), "/MergedConfig", json!({})),
            "/ContainerDetails/Config",
            json!({}),
        );
        assert_eq!(user(nobody), Ok(User::Root));
        let root = with(result(), "/MergedConfig/remoteUser", json!("root"));
        assert_eq!(user(root), Ok(User::Root));
    }

    #[test]
    fn a_user_devpod_would_respell_or_su_would_misread_takes_devpod_ssh() {
        for name in ["Vscode", "-c", "a b", "x'y", "1abc", "ünï"] {
            let result = with(result(), "/MergedConfig/remoteUser", json!(name));
            assert_eq!(
                target(&record(), &result, "ws"),
                Err(Decline::UnusualUser),
                "{name}"
            );
        }
    }

    #[test]
    fn a_folder_holding_a_nul_takes_devpod_ssh() {
        let result = with(
            result(),
            "/MergedConfig",
            json!({ "remoteUser": "vscode", "workspaceFolder": "/a\u{0}b" }),
        );
        assert_eq!(
            target(&record(), &result, "ws"),
            Err(Decline::UnusableWorkdir)
        );
    }

    #[test]
    fn the_envfile_becomes_quoted_exports_in_name_order() {
        let read = format!(
            "{ENVFILE_MARK}\n{}",
            r#"{"env":{"PATH":"/usr/bin:/bin","GH_TOKEN":"gho_x'y","PIXI_CACHE_DIR":"/var/tmp/p"}}"#
        );
        assert_eq!(
            exports(&read),
            Ok([
                r#"export GH_TOKEN='gho_x'"'"'y'"#,
                "export PATH=/usr/bin:/bin",
                "export PIXI_CACHE_DIR=/var/tmp/p",
            ]
            .join("\n"))
        );
    }

    /// devpod applies nothing when there is no file, and neither does this.
    #[test]
    fn no_envfile_is_no_exports() {
        assert_eq!(exports(&format!("{ENVFILE_MARK}\n")), Ok(String::new()));
        assert_eq!(exports(&format!("{ENVFILE_MARK}\n{{}}")), Ok(String::new()));
    }

    #[test]
    fn a_read_that_did_not_get_in_or_is_not_devpods_format_takes_devpod_ssh() {
        for read in [
            String::new(),
            "Error: No such container\n".to_owned(),
            format!("{ENVFILE_MARK}\nnot json"),
            format!("{ENVFILE_MARK}\n{{\"env\":[\"PATH\"]}}"),
            format!("{ENVFILE_MARK}\n{{\"env\":{{\"PATH\":7}}}}"),
            format!("{ENVFILE_MARK}\n{{\"env\":{{\"A-B\":\"x\"}}}}"),
            format!("{ENVFILE_MARK}\n{{\"env\":{{\"A\":\"x\\u0000y\"}}}}"),
        ] {
            assert_eq!(exports(&read), Err(Decline::Unreadable), "{read}");
        }
    }

    /// `su -c` hands the prelude to the user's own shell inside single quotes.
    #[test]
    fn the_prelude_survives_single_quotes_in_any_login_shell() {
        for forbidden in ['\'', '\\', '!', '\n'] {
            assert!(!PRELUDE.contains(forbidden), "{forbidden:?}");
        }
        assert!(PRELUDE.contains(ARRIVED_MARK));
        for name in [ENV_EXPORTS, ENV_WORKDIR, ENV_COMMAND] {
            assert!(PRELUDE.contains(name), "{name}");
        }
    }

    fn direct(user: User) -> Direct {
        Direct {
            target: Target {
                container: ID.to_owned(),
                user,
                workdir: "/workspaces/ws".to_owned(),
            },
            exports: "export GH_TOKEN=secret".to_owned(),
        }
    }

    /// A named user goes through `su` as devpod's session does, and nothing the
    /// envfile holds reaches argv.
    #[test]
    fn a_named_user_is_reached_through_su_with_the_secrets_kept_out_of_argv() {
        let spec = spec(&direct(User::Named("vscode".to_owned())), "bash -lc true");
        let argv = spec.invocation.argv();
        assert_eq!(
            argv,
            [
                "docker",
                "exec",
                "-i",
                "-u",
                "0",
                "-e",
                ENV_EXPORTS,
                "-e",
                ENV_WORKDIR,
                "-e",
                ENV_COMMAND,
                ID,
                "su",
                "-c",
                &format!("/bin/sh -c '{PRELUDE}'"),
                "vscode",
            ]
        );
        assert!(argv.iter().all(|arg| !arg.contains("secret")));
        let entries = &spec.invocation.env.entries;
        assert_eq!(entries[ENV_EXPORTS], "export GH_TOKEN=secret");
        assert_eq!(entries[ENV_WORKDIR], "/workspaces/ws");
        assert_eq!(entries[ENV_COMMAND], "bash -lc true");
    }

    /// devpod runs a root session without `su`, so its umask stays docker's.
    #[test]
    fn root_is_reached_without_su() {
        let argv = spec(&direct(User::Root), "true").invocation.argv();
        assert_eq!(&argv[argv.len() - 3..], ["/bin/sh", "-c", PRELUDE]);
        assert!(!argv.iter().any(|arg| arg == "su"));
    }

    #[test]
    fn arrival_is_the_mark_on_a_line_of_its_own() {
        assert!(arrived(&format!("motd\n{ARRIVED_MARK}\nmore\n")));
        assert!(!arrived(""));
        assert!(!arrived("su: user vscode does not exist\n"));
        assert!(!arrived(&format!("x{ARRIVED_MARK}\n")));
    }
}
