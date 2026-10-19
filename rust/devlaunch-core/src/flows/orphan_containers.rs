//! Containers a workspace left behind after devpod forgot it.
//!
//! `devpod delete` finds a workspace's container by one label,
//! `dev.containers.id=<uid>`, and composes the project down only if it finds that
//! container. Two ordinary events break the match and leave the compose project
//! running with nothing on the machine that still owns it:
//!
//! - **A forced delete.** `devpod delete --force` (what `dl <ws> kill` runs)
//!   logs a failed `compose down` and removes the record anyway. A compose file
//!   that no longer interpolates is enough: a required variable from a `.env`
//!   the `initializeCommand` wrote, gone from the clone.
//! - **A record recreated under a new uid.** A `devpod up` over leftover
//!   containers makes a new record with a new uid, then fails on the leftovers'
//!   fixed `container_name`. The next `devpod delete` looks for the *new* uid,
//!   finds no container, and reports success. The leftovers stay up.
//!
//! The leftovers then block every later `up` of the same workspace, because a
//! compose file that pins `container_name:` cannot make a second container under
//! the same name in a new project.
//!
//! # Which containers count
//!
//! Only containers of a workspace devpod holds **no record for in any context**.
//! With a record somewhere, devpod still owns the workspace and dl stays out of
//! its containers. A record dl cannot read counts as one: only a "not found"
//! everywhere a record could be is "no record". Without one, a container is this workspace's leftover when one
//! of its compose labels (`working_dir`, `config_files`) names a path inside:
//!
//! - devpod's agent-side directory for this workspace id, under any context.
//!   devpod writes its compose override there for every compose devcontainer,
//!   so this one handle names the workspace by id whatever uid made the project;
//! - or the workspace's clone, when it is one devlaunch made. A local folder
//!   opened with `dl ./folder` is the user's directory, and a compose project
//!   the user started there by hand is not dl's to remove, so a local folder is
//!   matched by the devpod directory alone.
//!
//! And a container is left whenever anything else may own it: a label that
//! names the devpod directory of a different id, or a path in a folder devpod
//! holds as another workspace's source inside the clone. When devpod's records
//! would not read, the clone alone matches nothing.
//!
//! Nothing is matched on a container *name*: names are the devcontainer's to
//! choose, and a pattern over them would reach other people's containers.

use std::path::Path;

use crate::clients::devpod_home::{DevpodHome, RecordPresence};
use crate::clients::docker::{self, ComposeContainer, Unlisted};
use crate::domain::workspace_state::NonEmpty;

/// What a sweep for one workspace's leftover containers came to.
///
/// Two silent arms and three that are worth a line, on the line every other
/// docker cleanup in dl draws: nothing to do and no docker to do it with are
/// silent; a removal, or a docker that would not deliver, is said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OrphanSweep {
    /// Nothing to remove: devpod still holds the workspace, no container names
    /// it, or there is no devpod home to check against.
    Nothing,
    /// No docker on this machine, so nothing was made to be left.
    NoDocker,
    /// These containers were removed, by name.
    Removed(NonEmpty<String>),
    /// docker would not list the containers, in its own words.
    NotListed { reason: String },
    /// docker would not remove these containers, in its own words.
    NotRemoved {
        names: NonEmpty<String>,
        reason: String,
    },
}

/// Remove the containers this workspace left behind, if devpod has forgotten it.
///
/// `clone` is the workspace's clone directory when devlaunch made it, and `None`
/// for anything else (see the module docs for why a local folder is not passed).
///
/// One `docker ps` when devpod holds no record for the id, and no docker call at
/// all when it does: the common case, a workspace devpod still knows, costs a
/// directory listing.
pub(crate) fn sweep(
    runner: &dyn crate::runner::Runner,
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
    clone: Option<&Path>,
) -> OrphanSweep {
    let Some(home) = devpod_home else {
        return OrphanSweep::Nothing;
    };
    if workspace_id.is_empty() || home.record_presence(workspace_id) != RecordPresence::Absent {
        return OrphanSweep::Nothing;
    }
    let containers = match docker::compose_containers(runner) {
        Ok(containers) => containers,
        Err(Unlisted::NotInstalled) => return OrphanSweep::NoDocker,
        Err(Unlisted::Failed(reason)) => return OrphanSweep::NotListed { reason },
    };
    let clone = clone.map(|path| CloneDir::of(path, home));
    let Some(left) = NonEmpty::of(
        containers
            .iter()
            .filter(|container| left_by(container, home, workspace_id, clone.as_ref())),
    ) else {
        return OrphanSweep::Nothing;
    };
    let names = left.map(|c| c.name.clone());
    let ids = left.map(|c| c.id.clone());
    match docker::remove_containers(runner, &ids) {
        docker::Answer::Ran { exit, .. } if exit.is_success() => OrphanSweep::Removed(names),
        docker::Answer::Ran { exit, stderr } => OrphanSweep::NotRemoved {
            names,
            reason: format!("docker rm failed ({exit:?}): {}", stderr.trim()),
        },
        docker::Answer::NotInstalled => OrphanSweep::NoDocker,
        docker::Answer::NotStarted(failure) => OrphanSweep::NotRemoved {
            names,
            reason: format!("docker rm did not finish ({:?})", failure.kind),
        },
    }
}

/// Whether one container's compose labels name this workspace, and nothing
/// else that could own it.
///
/// A container is left whenever any doubt remains:
///
/// - a label that names the devpod directory of a *different* workspace id
///   makes the container that workspace's, whatever else it names;
/// - a label inside the source folder of a workspace devpod holds, where that
///   folder is the clone or inside it, makes the container that workspace's: a
///   local folder opened as its own workspace can sit inside a clone;
/// - when devpod's records would not read, the clone alone proves nothing,
///   and only this id's devpod directory counts.
fn left_by(
    container: &ComposeContainer,
    home: &DevpodHome,
    workspace_id: &str,
    clone: Option<&CloneDir<'_>>,
) -> bool {
    let mut by_id = false;
    let mut in_clone = false;
    for path in container.paths() {
        match home.agent_dir_owner(path) {
            Some(owner) if owner == workspace_id => by_id = true,
            Some(_) => return false,
            None => {}
        }
        if let Some(clone) = clone {
            if clone
                .held_inside
                .as_ref()
                .is_some_and(|held| held.iter().any(|folder| path.starts_with(folder)))
            {
                return false;
            }
            in_clone |= path.starts_with(clone.path);
        }
    }
    by_id || (in_clone && clone.is_some_and(|clone| clone.held_inside.is_some()))
}

/// The workspace's clone, and the folders inside it that devpod holds as other
/// workspaces' sources (nothing when devpod's records would not say).
struct CloneDir<'a> {
    path: &'a Path,
    held_inside: Option<Vec<std::path::PathBuf>>,
}

impl<'a> CloneDir<'a> {
    fn of(path: &'a Path, home: &DevpodHome) -> Self {
        let held_inside = home.held_local_folders().map(|folders| {
            folders
                .into_iter()
                .filter(|folder| folder.starts_with(path))
                .collect()
        });
        Self { path, held_inside }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;
    use crate::clients::devpod_home::{agent_compose_file, devpod_home_with};

    const WS: &str = "kinisi-ros-ci-sim-shard-rebalance-cd1e";

    fn names<const N: usize>(names: [String; N]) -> NonEmpty<String> {
        NonEmpty::of(names).expect("at least one name")
    }

    /// One `docker ps` row in the template `compose_containers` asks for.
    fn row(id: &str, name: &str, working_dir: &Path, config_files: &[&Path]) -> String {
        let files: Vec<String> = config_files
            .iter()
            .map(|f| f.display().to_string())
            .collect();
        format!(
            "{id}\t{name}\t{}\t{}\n",
            working_dir.display(),
            files.join(",")
        )
    }

    fn clone_dir() -> PathBuf {
        PathBuf::from("/cache/devlaunch/repos/org/repo").join(WS)
    }

    /// The case from the field: devpod has no record, and the old project's two
    /// containers, the dev container and a sidecar from a second compose file,
    /// are both still there. Both go, in one call, and the sweep names them.
    #[test]
    fn a_forgotten_workspaces_containers_are_removed_by_their_compose_labels() {
        let home = devpod_home_with(&[]);
        let override_file = agent_compose_file(&home, "default", WS);
        let clone = clone_dir();
        let devcontainer = clone.join(".devcontainer");
        let compose = devcontainer.join("docker-compose.sim.x64.yml");
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(
                row(
                    "aaa",
                    &format!("kinisi_jazzy_{WS}"),
                    &devcontainer,
                    &[&compose, &override_file],
                ) + &row(
                    "bbb",
                    &format!("kinisi_unreal_runtime_1.6.1_{WS}"),
                    &devcontainer,
                    &[&compose, &override_file],
                ) + &row(
                    "ccc",
                    "someone_elses",
                    Path::new("/home/other/.devcontainer"),
                    &[Path::new("/home/other/.devcontainer/compose.yml")],
                ),
            ),
        );

        let swept = sweep(&fake, Some(&home), WS, Some(&clone));

        assert_eq!(
            swept,
            OrphanSweep::Removed(names([
                format!("kinisi_jazzy_{WS}"),
                format!("kinisi_unreal_runtime_1.6.1_{WS}"),
            ]))
        );
        assert_eq!(
            fake.args_to("docker").last().expect("a removal"),
            &["rm", "--force", "aaa", "bbb"]
        );
    }

    /// devpod still holds the workspace, so its containers are devpod's: dl does
    /// not even ask docker.
    #[test]
    fn a_workspace_devpod_still_holds_is_left_alone() {
        let home = devpod_home_with(&[("default", WS, Some(()))]);
        let fake = FakeRunner::new();

        assert_eq!(
            sweep(&fake, Some(&home), WS, Some(&clone_dir())),
            OrphanSweep::Nothing
        );
        assert!(fake.args_to("docker").is_empty(), "{:?}", fake.argvs());
    }

    /// A record dl cannot read may still be there, so it holds the workspace
    /// as surely as one dl can read: root-owned directories from container
    /// mounts are ordinary on real hosts.
    #[test]
    fn a_record_dl_cannot_read_still_holds_it() {
        // SAFETY: `geteuid` takes nothing, returns a uid and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipped: root reads a directory of mode 000");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let home = devpod_home_with(&[("default", WS, None)]);
        let dir = home
            .record("default", WS)
            .parent()
            .expect("a record directory")
            .to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))
            .expect("a locked record directory");
        let fake = FakeRunner::new();

        let swept = sweep(&fake, Some(&home), WS, Some(&clone_dir()));

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("the record directory unlocked again");
        assert_eq!(swept, OrphanSweep::Nothing);
        assert!(fake.args_to("docker").is_empty(), "{:?}", fake.argvs());
    }

    /// A context whose name is not UTF-8 is still a directory devpod may have
    /// written a record into.
    #[test]
    fn a_record_under_a_context_name_that_is_not_utf8_holds_it() {
        use std::os::unix::ffi::OsStrExt;
        let home = devpod_home_with(&[]);
        let record = home
            .path()
            .join("contexts")
            .join(std::ffi::OsStr::from_bytes(b"ctx\xff"))
            .join("workspaces")
            .join(WS)
            .join("workspace.json");
        std::fs::create_dir_all(record.parent().expect("a record directory"))
            .expect("a record directory");
        std::fs::write(&record, "{}").expect("a record");
        let fake = FakeRunner::new();

        assert_eq!(sweep(&fake, Some(&home), WS, None), OrphanSweep::Nothing);
        assert!(fake.args_to("docker").is_empty(), "{:?}", fake.argvs());
    }

    /// A record in any context counts, not only the one dl would pick.
    #[test]
    fn a_record_in_another_context_also_holds_it() {
        let home = devpod_home_with(&[("other", WS, None)]);
        let fake = FakeRunner::new();

        assert_eq!(sweep(&fake, Some(&home), WS, None), OrphanSweep::Nothing);
        assert!(fake.args_to("docker").is_empty());
    }

    /// The devpod directory is matched by whole path components: `ws` is not
    /// `ws-2`, and neither is a clone whose name only starts the same way.
    #[test]
    fn a_workspace_whose_id_only_starts_the_same_is_not_matched() {
        let home = devpod_home_with(&[]);
        let longer = format!("{WS}-2");
        let theirs = agent_compose_file(&home, "default", &longer);
        let their_clone = PathBuf::from(format!("{}-2", clone_dir().display()));
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(row(
                "ddd",
                &format!("kinisi_jazzy_{longer}"),
                &their_clone.join(".devcontainer"),
                &[&theirs],
            )),
        );

        assert_eq!(
            sweep(&fake, Some(&home), WS, Some(&clone_dir())),
            OrphanSweep::Nothing
        );
        assert_eq!(fake.args_to("docker").len(), 1, "a listing and no removal");
    }

    /// Without a clone to match, a container is matched by devpod's directory
    /// alone: a compose project somebody started by hand in their own folder
    /// names no devpod directory and stays.
    #[test]
    fn a_local_folder_is_matched_by_the_devpod_directory_only() {
        let home = devpod_home_with(&[]);
        let folder = PathBuf::from("/home/me/project");
        let override_file = agent_compose_file(&home, "default", "project");
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(
                row(
                    "eee",
                    "project_dev",
                    &folder.join(".devcontainer"),
                    &[&folder.join(".devcontainer/compose.yml"), &override_file],
                ) + &row("fff", "my_own_db", &folder, &[&folder.join("compose.yml")]),
            ),
        );

        assert_eq!(
            sweep(&fake, Some(&home), "project", None),
            OrphanSweep::Removed(names(["project_dev".to_owned()]))
        );
    }

    /// A project whose compose files all sit elsewhere still names the
    /// workspace when compose ran it from devpod's directory for the id.
    #[test]
    fn a_working_dir_in_the_devpod_directory_alone_is_enough() {
        let home = devpod_home_with(&[]);
        let agent_dir = agent_compose_file(&home, "default", WS)
            .parent()
            .expect("the compose override's directory")
            .to_path_buf();
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(row(
                "aaa",
                "dev",
                &agent_dir,
                &[Path::new("/srv/elsewhere/compose.yml")],
            )),
        );

        assert_eq!(
            sweep(&fake, Some(&home), WS, None),
            OrphanSweep::Removed(names(["dev".to_owned()]))
        );
        assert_eq!(
            fake.args_to("docker").last().expect("a removal"),
            &["rm", "--force", "aaa"]
        );
    }

    /// A local folder opened as its own workspace, `sub`, can sit inside another
    /// workspace's clone, `ws`. devpod still holds `sub`; it holds nothing for
    /// `ws`. Sweeping `ws` must not take `sub`'s live container just because its
    /// compose files are inside `ws`'s clone: its labels name `sub`'s devpod
    /// directory, so it is `sub`'s.
    #[test]
    fn a_held_workspace_nested_in_the_clone_keeps_its_containers() {
        let home = devpod_home_with(&[("default", "sub", None)]);
        let clone = PathBuf::from("/cache/devlaunch/repos/o/r/ws");
        let sub = clone.join("sub/.devcontainer");
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(row(
                "aaa",
                "sub_dev",
                &sub,
                &[
                    &sub.join("compose.yml"),
                    &agent_compose_file(&home, "default", "sub"),
                ],
            )),
        );

        assert_eq!(
            sweep(&fake, Some(&home), "ws", Some(&clone)),
            OrphanSweep::Nothing
        );
        assert_eq!(fake.args_to("docker").len(), 1, "a listing and no removal");
    }

    /// A container that names another workspace's devpod directory is that
    /// workspace's, even beside this one's: two owners is one too many to remove.
    #[test]
    fn a_container_naming_another_workspaces_devpod_directory_stays() {
        let home = devpod_home_with(&[]);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(row(
                "aaa",
                "dev",
                Path::new("/x"),
                &[
                    &agent_compose_file(&home, "default", WS),
                    &agent_compose_file(&home, "default", "other"),
                ],
            )),
        );

        assert_eq!(
            sweep(&fake, Some(&home), WS, Some(&clone_dir())),
            OrphanSweep::Nothing
        );
        assert_eq!(fake.args_to("docker").len(), 1, "a listing and no removal");
    }

    /// A compose project inside a folder devpod holds as a workspace's source is
    /// that workspace's business, even when the folder is inside this clone and
    /// the project names no devpod directory at all.
    #[test]
    fn a_project_in_a_held_workspaces_source_inside_the_clone_stays() {
        let home = devpod_home_with(&[("default", "sub", None)]);
        let clone = clone_dir();
        let sub = clone.join("sub");
        std::fs::write(
            home.record("default", "sub"),
            format!(r#"{{"source":{{"localFolder":"{}"}}}}"#, sub.display()),
        )
        .expect("a record with a source");
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(row("aaa", "their_db", &sub, &[&sub.join("compose.yml")])),
        );

        assert_eq!(
            sweep(&fake, Some(&home), WS, Some(&clone)),
            OrphanSweep::Nothing
        );
        assert_eq!(fake.args_to("docker").len(), 1, "a listing and no removal");
    }

    /// A held record whose source dl cannot read could name a folder inside the
    /// clone, so the clone alone proves nothing: only devpod's directory for the
    /// id still counts.
    #[test]
    fn an_unreadable_held_record_leaves_clone_only_matches() {
        let home = devpod_home_with(&[("default", "sub", None)]);
        std::fs::write(home.record("default", "sub"), "not json").expect("a broken record");
        let clone = clone_dir();
        let dir = clone.join(".devcontainer");
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(
                row(
                    "aaa",
                    "ours",
                    &dir,
                    &[&agent_compose_file(&home, "default", WS)],
                ) + &row("bbb", "maybe_theirs", &dir, &[&dir.join("b.yml")]),
            ),
        );

        assert_eq!(
            sweep(&fake, Some(&home), WS, Some(&clone)),
            OrphanSweep::Removed(names(["ours".to_owned()]))
        );
    }

    #[test]
    fn a_listing_docker_refuses_removes_nothing_and_keeps_docker_words() {
        let home = devpod_home_with(&[]);
        let fake = FakeRunner::new();
        fake.script(["docker", "ps"], Response::failed(1, "daemon is down\n"));

        let swept = sweep(&fake, Some(&home), WS, None);

        assert!(
            matches!(
                &swept,
                OrphanSweep::NotListed { reason } if reason.contains("daemon is down")
            ),
            "{swept:?}"
        );
        assert_eq!(fake.args_to("docker").len(), 1, "a listing and no removal");
    }

    #[test]
    fn no_devpod_home_means_nothing_to_check_against() {
        let fake = FakeRunner::new();

        assert_eq!(sweep(&fake, None, WS, None), OrphanSweep::Nothing);
        assert!(fake.argvs().is_empty());
    }

    #[test]
    fn a_machine_with_no_docker_is_silent() {
        let home = devpod_home_with(&[]);
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(sweep(&fake, Some(&home), WS, None), OrphanSweep::NoDocker);
    }

    #[test]
    fn a_removal_docker_refuses_keeps_the_names_and_docker_words() {
        let home = devpod_home_with(&[]);
        let override_file = agent_compose_file(&home, "default", WS);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "ps"],
            Response::stdout(row("aaa", "dev", Path::new("/x"), &[&override_file])),
        );
        fake.script(["docker", "rm"], Response::failed(1, "daemon said no\n"));

        assert_eq!(
            sweep(&fake, Some(&home), WS, None),
            OrphanSweep::NotRemoved {
                names: names(["dev".to_owned()]),
                reason: "docker rm failed (Code(1)): daemon said no".to_owned(),
            }
        );
    }
}
