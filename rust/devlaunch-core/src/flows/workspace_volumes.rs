//! The named docker volumes that belong to one workspace alone, read off its
//! containers before a delete and removed after it.
//!
//! [`super::lifecycle`]'s older sweep removes the two volumes whose *templates*
//! devlaunch knows (`${localWorkspaceFolderBasename}-pixi` and the
//! docker-in-docker feature's). Every other per-workspace volume — a compose
//! named volume over a build cache, a pixi volume a repository spells its own
//! way — outlived the workspace. This module reads what the workspace's
//! containers actually mount instead of guessing names, then keeps only the
//! volumes that are the workspace's own.
//!
//! # Which volumes go
//!
//! A volume one of the workspace's containers mounted, that **both**
//!
//! - **belongs to the workspace**: compose made it for the workspace's project
//!   under compose's default name `<project>_<volume>`, or its name holds the
//!   workspace id (or devpod's uid for it) as a whole `-`/`_`/`.`-separated
//!   part — what `${localWorkspaceFolderBasename}` and `${devcontainerId}`
//!   expand to;
//! - and **no container that is left uses** — asked of docker after the
//!   workspace's own containers are gone.
//!
//! A name the devcontainer config spells literally, with no substitution in it
//! (`team-tracker-pixi`), is shared by every checkout of that repository by
//! design and is never one workspace's, whatever the id. A compose volume with an
//! explicit `name:` is shared across projects for the same reason, which is why
//! a compose label alone is not enough.
//!
//! # Only the delete paths reach this
//!
//! [`super::lifecycle::workspace_delete`] is the one caller, and it sits behind
//! `rm`, `rme`, `kill` and `--rm`. `stop`, `up`, recreate and `--refresh-stale`
//! never reach it, so the caches a recreate is meant to keep stay put.

use std::collections::BTreeSet;
use std::path::Path;

use crate::clients::devpod_home::{DevpodHome, sole_workspace_record, sole_workspace_result};
use crate::clients::docker::{self, Unlisted, VolumeRemoval};
use crate::domain::workspace_state::NonEmpty;
use crate::runner::Runner;

/// What one workspace's containers mounted, read before its delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mounted {
    /// The workspace id dl addresses it by.
    pub(crate) workspace_id: String,
    /// devpod's uid for the workspace — `${devcontainerId}`, and the compose
    /// project devpod names after it.
    pub(crate) devcontainer_id: Option<String>,
    /// Every named volume any of the workspace's containers mounted.
    pub(crate) volumes: BTreeSet<String>,
    /// The volumes compose made for one of the workspace's projects under
    /// compose's own default name.
    pub(crate) compose_made: BTreeSet<String>,
    /// Volume names the devcontainer config spells with no substitution in them.
    pub(crate) literal: BTreeSet<String>,
}

/// What reading a workspace's mounts came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Recording {
    /// The mounts, read. `volumes` may be empty.
    Read(Mounted),
    /// No uid on record to find the containers by, or no docker: nothing to say.
    Nothing,
    /// docker would not answer, in its own words.
    Unread { reason: String },
}

/// What became of one volume after the delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Swept {
    /// docker removed it.
    Removed(String),
    /// Something else still uses it, so it stays.
    InUse(String),
    /// It is gone already.
    Gone(String),
    /// docker would not remove it, or would not say who uses it.
    NotRemoved { name: String, reason: String },
}

/// Whether `name` is this workspace's own volume, setting aside who uses it.
pub(crate) fn belongs(mounted: &Mounted, name: &str) -> bool {
    if mounted.literal.contains(name) {
        return false;
    }
    if mounted.compose_made.contains(name) {
        return true;
    }
    has_part(name, &mounted.workspace_id)
        || mounted
            .devcontainer_id
            .as_deref()
            .is_some_and(|uid| has_part(name, uid))
}

/// The volumes a delete should try to remove, in name order.
pub(crate) fn owned(mounted: &Mounted) -> Vec<String> {
    mounted
        .volumes
        .iter()
        .filter(|name| belongs(mounted, name))
        .cloned()
        .collect()
}

/// Whether `part` occurs in `name` bounded on both sides by the start or end of
/// the name or a separator, so `ws` is a part of `ws-pixi` but not of `ws2-pixi`.
fn has_part(name: &str, part: &str) -> bool {
    if part.is_empty() {
        return false;
    }
    let separator = |c: Option<char>| c.is_none_or(|c| matches!(c, '-' | '_' | '.'));
    name.match_indices(part).any(|(at, _)| {
        separator(name[..at].chars().next_back())
            && separator(name[at + part.len()..].chars().next())
    })
}

/// Read what the workspace's containers mount, before anything is removed.
///
/// The containers are found by devpod's uid for the workspace: the dev container
/// carries it as `dev.containers.id`, and devpod names the compose project after
/// it, so every compose sidecar carries it as `com.docker.compose.project`.
/// Stopped containers count: their volumes are as much the workspace's.
pub(crate) fn record(
    runner: &dyn Runner,
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Recording {
    let result = sole_workspace_result(devpod_home, workspace_id).and_then(|path| read_json(&path));
    let uid = sole_workspace_record(devpod_home, workspace_id)
        .and_then(|path| read_json(&path))
        .and_then(|record| non_empty(&record["uid"]))
        .or_else(|| {
            result
                .as_ref()
                .and_then(|r| non_empty(&r["SubstitutionContext"]["DevContainerID"]))
        });
    let Some(uid) = uid else {
        return Recording::Nothing;
    };
    let mut mounted = Mounted {
        workspace_id: workspace_id.to_owned(),
        devcontainer_id: Some(uid.clone()),
        literal: result.as_ref().map(literal_volumes).unwrap_or_default(),
        ..Mounted::default()
    };
    match read_mounts(runner, &uid, &mut mounted) {
        Ok(()) => Recording::Read(mounted),
        Err(Unlisted::NotInstalled) => Recording::Nothing,
        Err(Unlisted::Failed(reason)) => Recording::Unread { reason },
    }
}

/// The docker half of [`record`].
fn read_mounts(runner: &dyn Runner, uid: &str, mounted: &mut Mounted) -> Result<(), Unlisted> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    ids.extend(docker::containers_labelled(
        runner,
        &format!("dev.containers.id={uid}"),
    )?);
    ids.extend(docker::containers_labelled(
        runner,
        &format!("com.docker.compose.project={uid}"),
    )?);
    let Some(ids) = NonEmpty::of(ids) else {
        return Ok(());
    };
    let mut projects = BTreeSet::new();
    for container in docker::container_mounts(runner, &ids)? {
        projects.extend(container.project);
        mounted.volumes.extend(container.volumes);
    }
    if mounted.volumes.is_empty() {
        return Ok(());
    }
    for project in projects {
        for (name, key) in docker::compose_volumes(runner, &project)? {
            // Compose's default name. A volume with an explicit `name:` is
            // shared across projects on purpose, label or not.
            if name == format!("{project}_{key}") && mounted.volumes.contains(&name) {
                mounted.compose_made.insert(name);
            }
        }
    }
    Ok(())
}

/// Remove each owned volume nothing left uses, after the workspace's own
/// containers are gone. One volume at a time, so one refusal does not stop the
/// rest and each line names its own volume.
pub(crate) fn sweep(runner: &dyn Runner, mounted: &Mounted) -> Vec<Swept> {
    owned(mounted)
        .into_iter()
        .map(
            |name| match docker::containers_using_volume(runner, &name) {
                Ok(users) if !users.is_empty() => Swept::InUse(name),
                Ok(_) => match docker::remove_volume(runner, &name) {
                    VolumeRemoval::Removed => Swept::Removed(name),
                    VolumeRemoval::Gone => Swept::Gone(name),
                    VolumeRemoval::Refused(reason) => Swept::NotRemoved { name, reason },
                },
                Err(Unlisted::NotInstalled) => Swept::NotRemoved {
                    name,
                    reason: "docker is not installed".to_owned(),
                },
                Err(Unlisted::Failed(reason)) => Swept::NotRemoved { name, reason },
            },
        )
        .collect()
}

/// The volume sources the devcontainer config spells with no substitution in
/// them — shared by every checkout of the repository.
///
/// Read off the config as written (`DevContainerConfigWithPath`), not the merged
/// one, because only the written one still shows which names were templates.
fn literal_volumes(result: &serde_json::Value) -> BTreeSet<String> {
    let Some(mounts) = result["DevContainerConfigWithPath"]["config"]["mounts"].as_array() else {
        return BTreeSet::new();
    };
    mounts
        .iter()
        .filter_map(|mount| match mount {
            serde_json::Value::Object(_) => (mount["type"].as_str() == Some("volume"))
                .then(|| mount["source"].as_str().map(str::to_owned))
                .flatten(),
            serde_json::Value::String(spec) => {
                let field = |key: &str| {
                    spec.split(',').find_map(|part| {
                        let (k, v) = part.split_once('=')?;
                        (k.trim() == key).then(|| v.trim().to_owned())
                    })
                };
                (field("type").as_deref() == Some("volume"))
                    .then(|| field("source").or_else(|| field("src")))
                    .flatten()
            }
            _ => None,
        })
        .filter(|source| !source.is_empty() && !source.contains("${"))
        .collect()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn non_empty(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;
    use crate::clients::devpod_home::devpod_home_with;

    const WS: &str = "kinisi-ros-main-ab12";
    const UID: &str = "default-ki-65da1";
    /// The docker-in-docker feature's volume for UID.
    const DIND: &str = "dind-var-lib-docker-default-ki-65da1";

    fn mounted(volumes: &[&str]) -> Mounted {
        Mounted {
            workspace_id: WS.to_owned(),
            devcontainer_id: Some(UID.to_owned()),
            volumes: volumes.iter().map(|v| (*v).to_owned()).collect(),
            ..Mounted::default()
        }
    }

    #[test]
    fn a_volume_named_after_the_workspace_is_its_own() {
        let m = mounted(&[&format!("{WS}-pixi"), DIND]);
        assert_eq!(owned(&m), [DIND.to_owned(), format!("{WS}-pixi")]);
    }

    #[test]
    fn a_shared_volume_is_never_the_workspaces() {
        // team-tracker's devcontainer names its pixi and node_modules volumes
        // literally, so every checkout shares them.
        let mut m = mounted(&["team-tracker-pixi", "team-tracker-node-modules"]);
        m.workspace_id = "team-tracker-main-ab12".to_owned();
        assert_eq!(owned(&m), Vec::<String>::new());
    }

    #[test]
    fn a_literal_name_is_shared_even_when_the_id_is_a_part_of_it() {
        // A local folder opened as `team-tracker` has that bare id.
        let mut m = mounted(&["team-tracker-pixi"]);
        m.workspace_id = "team-tracker".to_owned();
        m.literal.insert("team-tracker-pixi".to_owned());
        assert_eq!(owned(&m), Vec::<String>::new());
    }

    #[test]
    fn the_id_must_be_a_whole_part_of_the_name() {
        let m = mounted(&[
            &format!("{WS}2-pixi"),
            &format!("x{WS}-pixi"),
            &format!("pixi_{WS}"),
        ]);
        assert_eq!(owned(&m), [format!("pixi_{WS}")]);
    }

    #[test]
    fn a_compose_volume_of_the_workspaces_project_is_its_own() {
        // kinisi_ros's `bazel-output` as compose names it for devpod's project.
        let name = format!("{UID}_bazel-output");
        let mut m = mounted(&[&name, "other-project_bazel-output", "kinisi-bazel-cache"]);
        m.compose_made.insert(name.clone());
        assert_eq!(owned(&m), [name]);
    }

    fn container_ps(label: &str) -> Vec<String> {
        [
            "docker",
            "ps",
            "--all",
            "--quiet",
            "--no-trunc",
            "--filter",
            label,
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// A devpod home whose record for WS carries UID.
    fn home_with_uid() -> crate::clients::devpod_home::ScratchHome {
        let home = devpod_home_with(&[("default", WS, None)]);
        std::fs::write(
            home.record("default", WS),
            serde_json::json!({ "id": WS, "uid": UID }).to_string(),
        )
        .expect("a record");
        home
    }

    #[test]
    fn the_mounts_of_the_dev_container_and_its_sidecars_are_read() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script(
            container_ps(&format!("label=dev.containers.id={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            container_ps(&format!("label=com.docker.compose.project={UID}")),
            Response::stdout("aaa\nbbb\n"),
        );
        fake.script(
            ["docker", "inspect"],
            Response::stdout(format!(
                "{UID}\t{WS}-pixi\t{UID}_bazel-output\t\n{UID}\tteam-cache\t\n"
            )),
        );
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(format!(
                "{UID}_bazel-output\tbazel-output\nnamed-cache\tcache\n"
            )),
        );

        let Recording::Read(m) = record(&fake, Some(&home), WS) else {
            panic!("read");
        };

        assert_eq!(
            m.volumes,
            [
                format!("{UID}_bazel-output"),
                format!("{WS}-pixi"),
                "team-cache".to_owned()
            ]
            .into_iter()
            .collect()
        );
        // `named-cache` has an explicit compose `name:`: shared by design.
        assert_eq!(
            m.compose_made,
            [format!("{UID}_bazel-output")].into_iter().collect()
        );
        assert_eq!(
            owned(&m),
            [format!("{UID}_bazel-output"), format!("{WS}-pixi")]
        );
        // Both containers inspected in one call.
        assert!(
            fake.argvs().iter().any(|argv| argv[1] == "inspect"
                && argv.ends_with(&["aaa".to_owned(), "bbb".to_owned()])),
            "{:?}",
            fake.argvs()
        );
    }

    #[test]
    fn a_workspace_with_no_record_reads_nothing_and_asks_docker_nothing() {
        let home = devpod_home_with(&[]);
        let fake = FakeRunner::new();

        assert_eq!(record(&fake, Some(&home), WS), Recording::Nothing);
        assert!(fake.argvs().is_empty());
    }

    #[test]
    fn a_machine_with_no_docker_reads_nothing() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(record(&fake, Some(&home), WS), Recording::Nothing);
    }

    #[test]
    fn a_docker_that_will_not_list_is_said() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script(["docker", "ps"], Response::failed(1, "daemon down\n"));

        assert!(matches!(
            record(&fake, Some(&home), WS),
            Recording::Unread { .. }
        ));
    }

    #[test]
    fn the_literal_names_come_from_the_devcontainer_config() {
        let home = home_with_uid();
        std::fs::write(
            home.result("default", WS),
            serde_json::json!({
                "DevContainerConfigWithPath": { "config": { "mounts": [
                    { "type": "volume", "source": "team-tracker-pixi", "target": "/x" },
                    { "type": "volume", "source": "${localWorkspaceFolderBasename}-pixi", "target": "/y" },
                    "source=team-tracker-node-modules,target=/z,type=volume",
                    { "type": "bind", "source": "/home", "target": "/h" },
                ]}},
                "SubstitutionContext": { "DevContainerID": UID },
            })
            .to_string(),
        )
        .expect("a result");
        let fake = FakeRunner::new();

        let Recording::Read(m) = record(&fake, Some(&home), WS) else {
            panic!("read");
        };
        assert_eq!(
            m.literal,
            ["team-tracker-node-modules", "team-tracker-pixi"]
                .map(str::to_owned)
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn a_volume_nothing_left_uses_is_removed_and_one_in_use_stays() {
        let m = mounted(&[&format!("{WS}-pixi"), &format!("{WS}-cache"), "shared"]);
        let fake = FakeRunner::new();
        fake.script(
            [
                "docker",
                "ps",
                "--all",
                "--quiet",
                "--no-trunc",
                "--filter",
                &format!("volume={WS}-cache"),
            ],
            Response::stdout("ccc\n"),
        );

        let swept = sweep(&fake, &m);

        assert_eq!(
            swept,
            [
                Swept::InUse(format!("{WS}-cache")),
                Swept::Removed(format!("{WS}-pixi")),
            ]
        );
        // `shared` is not the workspace's, so docker is never asked about it,
        // and the volume in use is never handed to `volume rm`.
        let removals: Vec<_> = fake
            .argvs()
            .into_iter()
            .filter(|argv| argv[1] == "volume")
            .collect();
        assert_eq!(
            removals,
            [["docker", "volume", "rm", &format!("{WS}-pixi")]]
        );
    }

    #[test]
    fn a_volume_docker_will_not_remove_is_reported_and_the_rest_still_go() {
        let m = mounted(&[&format!("{WS}-a"), &format!("{WS}-b")]);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "rm", &format!("{WS}-a")],
            Response::failed(1, "Error response from daemon: busy\n"),
        );
        fake.script(
            ["docker", "volume", "rm", &format!("{WS}-b")],
            Response::failed(1, "Error response from daemon: get x: no such volume\n"),
        );

        let swept = sweep(&fake, &m);

        assert!(
            matches!(&swept[0], Swept::NotRemoved { name, reason } if *name == format!("{WS}-a") && reason.contains("busy")),
            "{swept:?}"
        );
        assert_eq!(swept[1], Swept::Gone(format!("{WS}-b")));
    }

    #[test]
    fn a_name_is_a_part_only_between_separators() {
        assert!(has_part("ws-pixi", "ws"));
        assert!(has_part("a.ws", "ws"));
        assert!(!has_part("ws2-pixi", "ws"));
        assert!(!has_part("anything", ""));
    }
}
