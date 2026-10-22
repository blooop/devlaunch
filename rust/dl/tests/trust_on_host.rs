//! A cold launch records the new workspace as trusted in the host's Claude config,
//! even when the container has no `python3`.
//!
//! The shape this pins is the default one: devlaunch's claude-code feature mounts the
//! host's `~/.claude` read-write into the container and pins `CLAUDE_CONFIG_DIR` at
//! it, so the `.claude.json` the container's Claude Code reads is the host's. The
//! trust stage used to merge `projects[<workspace>].hasTrustDialogAccepted` into it
//! with `python3` in the container, and `ghcr.io/blooop/python_template` has no
//! `python3`, so every new workspace asked "Do you trust the files in this folder?".
//!
//! The core suite pins the stage's report and the host merge
//! (`flows::provision`'s
//! `a_container_without_python3_sharing_the_hosts_config_ends_up_trusted`). This
//! one pins the launch: the real `dl` binary, a cold `up`, a `devpod` whose
//! `ssh --command` runs the pass for real in a "container" directory tree under a
//! PATH with no `python3`, and a `docker` that says how that tree is mounted.
//!
//! The container's config directory is a separate directory here rather than a
//! real bind mount, so a write that lands in the host's file can only have been made
//! on the host.
//!
//! Linux-only, like `dl/tests/setup_pass_shells.rs`, whose cold world this borrows.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use devlaunch_test_support::KeepingCoverage;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// The id `dl blooop/devlaunch@cold` derives, as `dl/tests/launch.rs` has it.
const COLD: &str = "devlaunch-cold-8iyb";

/// What the host's Claude Code had written before the launch.
const HOST_CONFIG: &str = "{\n  \"hasCompletedOnboarding\": true,\n  \"themeMode\": \"dark\"\n}\n";

struct World {
    root: PathBuf,
    /// The host's `$CLAUDE_CONFIG_DIR`, relative to `root`, where it sets one.
    claude_config_dir: Option<&'static str>,
    _scratch: tempfile::TempDir,
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("a program");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("keep it executable");
}

impl World {
    fn cold() -> Self {
        Self::cold_with_host_claude_config_dir(None)
    }

    /// A cold world whose host sets `CLAUDE_CONFIG_DIR` to `dir` under the root, and
    /// whose container mounts that directory rather than `~/.claude`.
    fn cold_with_host_claude_config_dir(claude_config_dir: Option<&'static str>) -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dltrust")
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch
            .path()
            .canonicalize()
            .expect("a resolved scratch dir");
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/launch_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "launch_scenario.py failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        // The host: its Claude config directory, holding its real config.
        std::fs::create_dir_all(root.join("home/.claude")).expect("a host claude dir");
        std::fs::write(root.join("home/.claude/.claude.json"), HOST_CONFIG)
            .expect("the host's config");
        let host_claude = root.join(claude_config_dir.unwrap_or("home/.claude"));
        std::fs::create_dir_all(&host_claude).expect("the host's config dir");
        std::fs::write(host_claude.join(".claude.json"), HOST_CONFIG).expect("the host's config");

        // The container: a config directory holding what the mount shows, and the
        // workspace folder the pass stands in.
        let ctr_claude = root.join("ctr/home/vscode/.claude");
        let ctr_workspace = root.join("ctr/workspaces").join(COLD);
        std::fs::create_dir_all(&ctr_claude).expect("a container claude dir");
        std::fs::create_dir_all(ctr_workspace.join(".git")).expect("a workspace folder");
        std::fs::write(ctr_claude.join(".claude.json"), HOST_CONFIG).expect("the mount's view");

        // The container's PATH: a shell and the coreutils the pass uses, and no
        // python3. `sudo` is a stand-in so the hostname stage renames nothing.
        let nopy = root.join("ctr/bin");
        std::fs::create_dir_all(&nopy).expect("a container bin");
        for tool in ["bash", "sh", "readlink", "mkdir", "awk", "grep", "cat"] {
            let found = ["/usr/bin", "/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(tool))
                .find(|path| path.exists());
            if let Some(found) = found {
                std::os::unix::fs::symlink(found, nopy.join(tool)).expect("a linked tool");
            }
        }
        executable(&nopy.join("sudo"), "#!/bin/sh\nexit 0\n");
        assert!(!nopy.join("python3").exists());

        // devpod: `up` records the container, the clone it mounted and the
        // workspace folder in the container, as real devpod does in
        // workspace_result.json; `ssh --command` runs the pass in the container
        // tree.
        let devpod = root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        let run = delegate.strip_prefix("exec ").expect("an exec line");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"up\" ]; then\n\
             \x20 {run} \"$@\" || exit $?\n\
             \x20 ws=$2; prev=\n\
             \x20 for a in \"$@\"; do\n\
             \x20   if [ \"$prev\" = \"--id\" ]; then ws=$a; fi\n\
             \x20   prev=$a\n\
             \x20 done\n\
             \x20 d=\"$DEVPOD_HOME/contexts/default/workspaces/$ws\"\n\
             \x20 mkdir -p \"$d\"\n\
             \x20 printf '{{}}' > \"$d/workspace.json\"\n\
             \x20 printf '%s' \"$2\" > \"{root}/clone-path\"\n\
             \x20 printf '{{\"ContainerDetails\":{{\"ID\":\"c0ffee\"}},\"SubstitutionContext\":{{\"LocalWorkspaceFolder\":\"%s\",\"ContainerWorkspaceFolder\":\"{ws}\"}}}}' \"$2\" > \"$d/workspace_result.json\"\n\
             \x20 exit 0\n\
             fi\n\
             if [ \"$1\" = \"ssh\" ]; then\n\
             \x20 cmd=; prev=\n\
             \x20 for a in \"$@\"; do\n\
             \x20   if [ \"$prev\" = \"--command\" ]; then cmd=$a; fi\n\
             \x20   prev=$a\n\
             \x20 done\n\
             \x20 if [ -n \"$cmd\" ]; then\n\
             \x20   {run} \"$@\" >/dev/null || exit $?\n\
             \x20   cd \"{ws}\" || exit 1\n\
             \x20   exec env -i HOME=\"{home}\" CLAUDE_CONFIG_DIR=\"{cfg}\" PATH=\"{nopy}\" sh -c \"$cmd\"\n\
             \x20 fi\n\
             fi\n\
             {delegate}\n",
            root = root.display(),
            ws = ctr_workspace.display(),
            home = root.join("ctr/home/vscode").display(),
            cfg = ctr_claude.display(),
            nopy = nopy.display(),
        );
        executable(&devpod, &script);

        // docker: the mounts of the one container devpod recorded. Anything else
        // it is asked is answered with nothing.
        executable(
            &root.join("bin/docker"),
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = inspect ] && [ \"$5\" = '{{{{json .Mounts}}}}' ] && [ \"$6\" = c0ffee ]; then\n\
                 \x20 clone=$(cat \"{root}/clone-path\")\n\
                 \x20 printf '[{{\"Type\":\"bind\",\"Source\":\"%s\",\"Destination\":\"{ws}\",\"RW\":true}},{{\"Type\":\"bind\",\"Source\":\"{host_claude}\",\"Destination\":\"{cfg}\",\"RW\":true}}]\\n' \"$clone\"\n\
                 fi\n\
                 exit 0\n",
                root = root.display(),
                ws = ctr_workspace.display(),
                cfg = ctr_claude.display(),
                host_claude = host_claude.display(),
            ),
        );

        World {
            root,
            claude_config_dir,
            _scratch: scratch,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A cold `dl blooop/devlaunch@cold`, run to completion, and what it said.
    fn launched(&self) -> String {
        let root = self.root.display().to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        command.arg("blooop/devlaunch@cold").env_clear();
        if let Some(dir) = self.claude_config_dir {
            command.env("CLAUDE_CONFIG_DIR", self.root.join(dir));
        }
        let done = command
            .keeping_coverage()
            .env("PATH", format!("{root}/bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
            .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
            .env("DEVLAUNCH_NO_GH_TOKEN", "1")
            .env("DEVLAUNCH_NO_TOOLS", "1")
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("the dl binary runs");
        let said = String::from_utf8_lossy(&done.stderr).into_owned();
        assert!(
            done.status.success(),
            "dl failed ({:?}): {said}",
            done.status.code()
        );
        said
    }

    fn config(&self, relative: &str) -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.path(relative)).expect("a config to read"),
        )
        .expect("JSON Claude Code can read")
    }
}

#[test]
fn a_cold_launch_without_python3_in_the_container_trusts_the_workspace_on_the_host() {
    let world = World::cold();
    let said = world.launched();

    let workspace = world.path("ctr/workspaces").join(COLD);
    let key = workspace
        .to_str()
        .expect("a path")
        .replace('~', "~0")
        .replace('/', "~1");
    let host = world.config("home/.claude/.claude.json");
    assert_eq!(
        host.pointer(&format!("/projects/{key}/hasTrustDialogAccepted")),
        Some(&true.into()),
        "the host's config has no trust entry for {}: {host}\n{said}",
        workspace.display()
    );
    assert_eq!(
        host.get("themeMode"),
        Some(&"dark".into()),
        "and keeps the rest"
    );
    assert_eq!(
        world.config("ctr/home/vscode/.claude/.claude.json"),
        serde_json::from_str::<serde_json::Value>(HOST_CONFIG).expect("JSON"),
        "nothing in the container wrote it"
    );
}

#[test]
fn a_cold_launch_trusts_the_workspace_in_the_hosts_own_claude_config_dir() {
    let world = World::cold_with_host_claude_config_dir(Some("profiles/work"));
    let said = world.launched();

    let key = world
        .path("ctr/workspaces")
        .join(COLD)
        .to_str()
        .expect("a path")
        .replace('~', "~0")
        .replace('/', "~1");
    let host = world.config("profiles/work/.claude.json");
    assert_eq!(
        host.pointer(&format!("/projects/{key}/hasTrustDialogAccepted")),
        Some(&true.into()),
        "the config dir the host's CLAUDE_CONFIG_DIR names has no trust entry: {host}\n{said}"
    );
    assert_eq!(
        world.config("home/.claude/.claude.json"),
        serde_json::from_str::<serde_json::Value>(HOST_CONFIG).expect("JSON"),
        "the default config dir is not the one the host uses"
    );
}
