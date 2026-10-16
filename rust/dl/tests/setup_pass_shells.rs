//! How many shells a cold launch's setup pass starts inside the container.
//!
//! The pass is one `devpod ssh --command` trip carrying dl's stages and then the
//! probe. Every new bash in it reads the image's `$BASH_ENV`, and an image may point
//! that at a heavy shell profile (for example a ROS environment). One such image
//! costs about 1.1s a read inside a `devpod ssh` session. When each stage was a nested `bash -c`, a cold launch paid
//! that read once for the pass and once more per stage, 11s for the trip where 6.7s
//! does the same work. The stages now run in subshells of the pass.
//!
//! The core suite pins the script (`flows::provision`'s
//! `the_pass_reads_the_images_bash_env_once_however_many_stages_it_runs`). This
//! one pins the launch: the real `dl` binary, a cold `up`, and a `devpod` whose
//! `ssh --command` runs the command it was handed under a `BASH_ENV` that counts
//! its reads, the way the container's shell would.
//!
//! Linux-only, like `dl/tests/resumed_setup.rs`, whose cold world this borrows.

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

struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    /// The cold world `dl/tests/launch.rs` builds, with a `devpod` that runs the
    /// setup pass's command for real.
    ///
    /// The scenario's fake devpod answers `ssh` by exiting 0 and runs nothing, so a
    /// count over it would be zero whatever the pass sent. This wrapper runs the
    /// `--command` through `sh -c`, as devpod's ssh server hands it to the user's
    /// shell, and then lets the fake keep its own books.
    fn cold() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dlshells")
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
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

        let devpod = root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        let run = delegate.strip_prefix("exec ").expect("an exec line");
        // The `up` half is `resumed_setup.rs`'s: devpod's create records, which the
        // launch reads on the way to the pass.
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
             \x20 printf '{{}}' > \"$d/workspace_result.json\"\n\
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
             \x20   BASH_ENV=\"$DL_BASH_ENV\" exec sh -c \"$cmd\"\n\
             \x20 fi\n\
             fi\n\
             {delegate}\n"
        );
        std::fs::write(&devpod, script).expect("rewrite devpod");
        // The hostname stage runs `sudo hostname <id>`, and the test host's sudo
        // must neither prompt nor rename the machine running the suite.
        let sudo = root.join("bin/sudo");
        std::fs::write(&sudo, "#!/bin/sh\nexit 0\n").expect("a fake sudo");
        let reads = root.join("bash-env-reads");
        std::fs::write(
            root.join("bash-env"),
            format!("echo read >> \"{}\"\n", reads.display()),
        )
        .expect("a counting BASH_ENV");
        use std::os::unix::fs::PermissionsExt as _;
        for program in [&devpod, &sudo] {
            std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755))
                .expect("keep it executable");
        }

        World {
            root,
            _scratch: scratch,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A cold `dl blooop/devlaunch@cold`, run to completion.
    ///
    /// `DEVLAUNCH_NO_TOOLS` so the pass carries its stages and the probe and stops
    /// there: the stages are not tools work, so they all still run, and the lend
    /// and the network install that would follow do not.
    fn launched(&self) -> String {
        let root = self.root.display().to_string();
        let done = Command::new(env!("CARGO_BIN_EXE_dl"))
            .arg("blooop/devlaunch@cold")
            .env_clear()
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
            .env("DL_BASH_ENV", self.path("bash-env").display().to_string())
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("the dl binary runs");
        assert!(
            done.status.success(),
            "dl failed ({:?}): {}",
            done.status.code(),
            String::from_utf8_lossy(&done.stderr)
        );
        String::from_utf8_lossy(&done.stderr).into_owned()
    }

    fn bash_env_reads(&self) -> usize {
        std::fs::read_to_string(self.path("bash-env-reads"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

#[test]
fn a_cold_launch_reads_the_images_bash_env_once_for_the_whole_setup_pass() {
    let world = World::cold();
    let said = world.launched();

    // The pass reached the container and every stage reported back: a stage that
    // never ran reads nothing, so a count of one is only worth something beside a
    // pass with nothing missing from it.
    assert!(
        !said.contains("not reached") && !said.contains("did not finish"),
        "the setup pass did not run whole: {said}"
    );
    assert!(
        world.path("home/.claude.json").exists(),
        "the onboarding stage did not run, so the count proves nothing: {said}"
    );
    // One read: the pass's own `bash -lc`. Each stage that started a bash of its
    // own read it again.
    assert_eq!(
        world.bash_env_reads(),
        1,
        "the setup pass read BASH_ENV {} times: {said}",
        world.bash_env_reads()
    );
}
