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
//!   under compose's default name `<project>_<volume>`, or its name holds one
//!   of devpod's uids for it as a whole `-`/`_`/`.`-separated part — what
//!   `${devcontainerId}` expands to — or it holds the workspace id that way —
//!   what `${localWorkspaceFolderBasename}` expands to — and a container of the
//!   current uid mounts it, started from the config dl read (see
//!   [`Mounted::config_read`]);
//! - and **no container that is left uses** — asked of docker after the
//!   workspace's own containers are gone.
//!
//! A compose volume of one of the workspace's projects is found by compose's
//! own label as well as by its mounts, so it goes even when no container is left
//! to mount it. The projects are devpod's uids for the workspace: the current
//! one, the ones devlaunch kept from earlier `up`s
//! ([`super::kept_copies::KeptCopies::uids`]), and the projects of the leftover
//! containers a delete removed ([`record_projects`]). Each uid is a value devpod
//! wrote down, so this is not a pattern over `docker volume ls`.
//!
//! A name the devcontainer config spells literally, with no substitution in it
//! (`team-tracker-pixi`), is shared by every checkout of that repository by
//! design and is never one workspace's, whatever the id. A compose volume with an
//! explicit `name:` (one compose labelled with a name other than its default
//! `<project>_<volume>`) is shared across projects for the same reason, and so is a
//! volume the compose file marks `external: true`: compose did not make it, so it
//! carries no compose label, and it is read off the compose files instead (the
//! ones the containers' `com.docker.compose.project.config_files` label names,
//! and the files those `include:`).
//! A compose file that cannot be read, or an external name or included path
//! spelled with a variable, makes the whole read [`Recording::Unread`]: no volume of that
//! workspace is removed beyond the two templates, because keeping a volume is
//! the mistake that can be undone.
//!
//! # Only the delete paths reach this
//!
//! [`super::lifecycle::workspace_delete`] is the one caller, and it sits behind
//! `rm`, `rme`, `kill` and `--rm`. `stop`, `up`, recreate and `--refresh-stale`
//! never reach it, so the caches a recreate is meant to keep stay put.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::clients::compose_files::ComposeFiles;
use crate::clients::devpod_home::{
    DevpodHome, RecordPresence, sole_workspace_record, sole_workspace_result,
};
use crate::clients::docker::{self, Unlisted, VolumeRemoval};
use crate::domain::workspace_state::NonEmpty;
use crate::runner::Runner;

/// What one workspace's containers mounted, read before its delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mounted {
    /// The workspace id dl addresses it by.
    pub(crate) workspace_id: String,
    /// devpod's uids for the workspace — `${devcontainerId}`, and the compose
    /// projects devpod names after them: the current one and every older one.
    pub(crate) uids: BTreeSet<String>,
    /// Every named volume any of the workspace's containers mounted.
    pub(crate) volumes: BTreeSet<String>,
    /// The volumes the containers of the current uid mount, when devpod's create
    /// result for it was read. Only for these does [`Self::literal`] say which
    /// names the config spells literally, so only these can be claimed by the
    /// workspace id in their name. A volume only an older uid's containers mount
    /// came from an older config, which nothing on the machine still holds.
    pub(crate) config_read: BTreeSet<String>,
    /// The volumes compose made for one of the workspace's projects under
    /// compose's own default name.
    pub(crate) compose_made: BTreeSet<String>,
    /// Volume names spelled on purpose: with no substitution in the devcontainer
    /// config, as an explicit compose `name:`, or as a compose volume marked
    /// `external`.
    pub(crate) literal: BTreeSet<String>,
}

/// What reading a workspace's mounts came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Recording {
    /// The mounts, read. `volumes` may be empty.
    Read(Mounted),
    /// No uid on record to find the containers by, or no docker: nothing to say.
    Nothing,
    /// docker would not answer, in its own words, or a compose file the
    /// containers were started from could not be read.
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
    mounted.uids.iter().any(|uid| has_part(name, uid))
        || (mounted.config_read.contains(name) && has_part(name, &mounted.workspace_id))
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
///
/// `former_uids` are the uids devlaunch kept from earlier `up`s of the same
/// workspace ([`super::kept_copies::KeptCopies::uids`]). devpod makes a record
/// again under a new uid, and the old project's containers and volumes still
/// carry the old one, so each is read as the workspace's too.
///
/// A compose volume of one of these projects, under compose's default name, is
/// the workspace's own **whether or not a container is left to mount it**: a
/// workspace whose containers are gone (dl shows it `NotFound`) still has its
/// `<uid>_bazel-output`, and before this the read found no container and stopped
/// there.
///
/// The kept uids are keyed by the workspace id alone, so they are read only when
/// one devpod context at most holds that id: with two, a kept uid may be the
/// other context's workspace, which is not being deleted.
pub(crate) fn record(
    runner: &dyn Runner,
    devpod_home: Option<&DevpodHome>,
    former_uids: &[String],
    workspace_id: &str,
) -> Recording {
    let result = sole_workspace_result(devpod_home, workspace_id).and_then(|path| read_json(&path));
    let uid = uid_in(devpod_home, workspace_id, result.as_ref());
    let one_owner = devpod_home.is_some_and(|home| {
        sole_workspace_record(Some(home), workspace_id).is_some()
            || home.record_presence(workspace_id) == RecordPresence::Absent
    });
    let former_uids: &[String] = if one_owner { former_uids } else { &[] };
    let uids: BTreeSet<String> = uid
        .iter()
        .chain(former_uids)
        .filter(|uid| !uid.is_empty())
        .cloned()
        .collect();
    if uids.is_empty() {
        return Recording::Nothing;
    }
    let config = Config {
        current: uid.filter(|_| result.is_some()),
    };
    let mounted = Mounted {
        workspace_id: workspace_id.to_owned(),
        uids: uids.clone(),
        literal: result.as_ref().map(literal_volumes).unwrap_or_default(),
        ..Mounted::default()
    };
    read(runner, &uids, &config, mounted)
}

/// Which uid's containers were started from the config dl read.
#[derive(Default)]
struct Config {
    /// The current uid, when its create result was read; nothing otherwise.
    current: Option<String>,
}

/// The compose volumes of `projects`, the compose projects of containers a delete
/// removed as this workspace's leftovers ([`super::orphan_containers`]).
///
/// Read after those containers are gone, so only compose's volume labels are left
/// to read: a volume of one of these projects under compose's default name. A
/// leftover's project is an older uid of the workspace that devlaunch may hold no
/// copy of, so without this its volumes outlived the containers that were removed.
pub(crate) fn record_projects(
    runner: &dyn Runner,
    workspace_id: &str,
    projects: &BTreeSet<String>,
) -> Recording {
    if projects.is_empty() {
        return Recording::Nothing;
    }
    let mounted = Mounted {
        workspace_id: workspace_id.to_owned(),
        uids: projects.clone(),
        ..Mounted::default()
    };
    read(runner, projects, &Config::default(), mounted)
}

fn read(
    runner: &dyn Runner,
    uids: &BTreeSet<String>,
    config: &Config,
    mut mounted: Mounted,
) -> Recording {
    match read_mounts(runner, uids, config, &mut mounted) {
        Ok(()) => Recording::Read(mounted),
        Err(NotRead::Docker(Unlisted::NotInstalled)) => Recording::Nothing,
        Err(NotRead::Docker(Unlisted::Failed(reason)) | NotRead::ComposeFile(reason)) => {
            Recording::Unread { reason }
        }
    }
}

/// devpod's uid for the workspace: the compose project devpod names after it, and
/// the `dev.containers.id` its dev container carries. From devpod's record, or
/// from its create result where the record has none.
pub(crate) fn workspace_uid(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Option<String> {
    let result = sole_workspace_result(devpod_home, workspace_id).and_then(|path| read_json(&path));
    uid_in(devpod_home, workspace_id, result.as_ref())
}

fn uid_in(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
    result: Option<&serde_json::Value>,
) -> Option<String> {
    sole_workspace_record(devpod_home, workspace_id)
        .and_then(|path| read_json(&path))
        .and_then(|record| non_empty(&record["uid"]))
        .or_else(|| result.and_then(|r| non_empty(&r["SubstitutionContext"]["DevContainerID"])))
}

/// Why [`read_mounts`] could not say which volumes are the workspace's.
enum NotRead {
    /// docker is not there, or did not answer.
    Docker(Unlisted),
    /// A compose file the containers were started from, or one it includes,
    /// could not be read for its external volumes, in these words.
    ComposeFile(String),
}

impl From<Unlisted> for NotRead {
    fn from(unlisted: Unlisted) -> Self {
        Self::Docker(unlisted)
    }
}

/// The docker half of [`record`].
fn read_mounts(
    runner: &dyn Runner,
    uids: &BTreeSet<String>,
    config: &Config,
    mounted: &mut Mounted,
) -> Result<(), NotRead> {
    let mut current: BTreeSet<String> = BTreeSet::new();
    let mut former: BTreeSet<String> = BTreeSet::new();
    for uid in uids {
        let ids = if config.current.as_ref() == Some(uid) {
            &mut current
        } else {
            &mut former
        };
        ids.extend(docker::containers_labelled(
            runner,
            &format!("dev.containers.id={uid}"),
        )?);
        ids.extend(docker::containers_labelled(
            runner,
            &format!("com.docker.compose.project={uid}"),
        )?);
    }
    former.retain(|id| !current.contains(id));
    let mut groups = ComposeGroups::default();
    if let Some(ids) = NonEmpty::of(current) {
        let volumes = read_containers(runner, &ids, &mut groups)?;
        mounted.config_read.extend(volumes.iter().cloned());
        mounted.volumes.extend(volumes);
    }
    if let Some(ids) = NonEmpty::of(former) {
        mounted
            .volumes
            .extend(read_containers(runner, &ids, &mut groups)?);
    }
    if !mounted.volumes.is_empty() {
        for group in groups.by_project.values().chain(&groups.projectless) {
            mounted
                .literal
                .extend(group.external_volumes().map_err(NotRead::ComposeFile)?);
        }
    }
    let project_files = groups.by_project;
    for volume in docker::compose_volumes(runner)? {
        let default_name = volume.name == format!("{}_{}", volume.project, volume.key);
        if default_name && uids.contains(&volume.project) {
            // Compose made it for one of the workspace's own projects. Its label
            // says so whether or not a container is left to mount it.
            mounted.volumes.insert(volume.name.clone());
            mounted.compose_made.insert(volume.name);
            continue;
        }
        if !mounted.volumes.contains(&volume.name) {
            continue;
        }
        if !default_name {
            mounted.literal.insert(volume.name);
        } else if project_files.contains_key(&volume.project) {
            mounted.compose_made.insert(volume.name);
        }
    }
    Ok(())
}

/// The compose files the workspace's containers were started from, read for the
/// external volumes they declare.
#[derive(Default)]
struct ComposeGroups {
    by_project: BTreeMap<String, ComposeFiles>,
    /// docker sets each compose label on its own, so a container can name
    /// compose files with no project. Its files are read as a group of their
    /// own: dropping them would find fewer external volumes.
    projectless: Vec<ComposeFiles>,
}

/// The container half of [`read_mounts`]: what `ids` mount, with the compose
/// files they were started from added to `groups`.
fn read_containers(
    runner: &dyn Runner,
    ids: &NonEmpty<String>,
    groups: &mut ComposeGroups,
) -> Result<BTreeSet<String>, NotRead> {
    let mut volumes = BTreeSet::new();
    for container in docker::container_mounts(runner, ids)? {
        match container.project {
            Some(project) => groups
                .by_project
                .entry(project)
                .or_default()
                .merge(container.compose),
            None if !container.compose.files.is_empty() => {
                groups.projectless.push(container.compose);
            }
            None => {}
        }
        volumes.extend(container.volumes);
    }
    Ok(volumes)
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
            uids: [UID.to_owned()].into_iter().collect(),
            volumes: volumes.iter().map(|v| (*v).to_owned()).collect(),
            config_read: volumes.iter().map(|v| (*v).to_owned()).collect(),
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

    /// One container of compose project `project`, started from `compose`, that
    /// mounts `volumes`.
    fn inspected(
        project: &str,
        compose: ComposeFiles,
        volumes: &[&str],
    ) -> docker::ContainerMounts {
        docker::ContainerMounts {
            project: Some(project.to_owned()),
            compose,
            volumes: volumes.iter().map(|v| (*v).to_owned()).collect(),
        }
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

    /// [`home_with_uid`], with the create result devpod writes at the end of an
    /// `up`: the config read, with no literal volume in it.
    fn home_with_result() -> crate::clients::devpod_home::ScratchHome {
        let home = home_with_uid();
        write_result(&home, "default", WS);
        home
    }

    fn write_result(home: &DevpodHome, context: &str, workspace_id: &str) {
        std::fs::write(
            home.result(context, workspace_id),
            serde_json::json!({
                "DevContainerConfigWithPath": { "config": { "mounts": [] } },
                "SubstitutionContext": { "DevContainerID": UID },
            })
            .to_string(),
        )
        .expect("a result");
    }

    #[test]
    fn the_mounts_of_the_dev_container_and_its_sidecars_are_read() {
        let home = home_with_result();
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
            Response::stdout(docker::inspect_output(&[
                inspected(
                    UID,
                    ComposeFiles::default(),
                    &[&format!("{WS}-pixi"), &format!("{UID}_bazel-output")],
                ),
                inspected(UID, ComposeFiles::default(), &["team-cache"]),
            ])),
        );
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(format!(
                "{UID}_bazel-output\t{UID}\tbazel-output\nnamed-cache\t{UID}\tcache\n"
            )),
        );

        let Recording::Read(m) = record(&fake, Some(&home), &[], WS) else {
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

    /// The volume ls rows for the case from the field: the workspace's own
    /// `bazel-output`, a volume compose named on purpose for the same project,
    /// and another project's `bazel-output`.
    fn bazel_output_rows(project: &str) -> String {
        format!(
            "{project}_bazel-output\t{project}\tbazel-output\n\
             named-cache\t{project}\tcache\n\
             default-ki-0ther_bazel-output\tdefault-ki-0ther\tbazel-output\n"
        )
    }

    /// dl shows the workspace NotFound: its containers are gone, so nothing
    /// mounts its compose volume. The volume still carries compose's project
    /// label, and the project is the workspace's uid, so it is still the
    /// workspace's own. Before this, the read found no container and stopped,
    /// and every such delete left `<uid>_bazel-output` behind.
    #[test]
    fn a_compose_volume_is_the_workspaces_own_when_no_container_is_left() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(bazel_output_rows(UID)),
        );

        let Recording::Read(m) = record(&fake, Some(&home), &[], WS) else {
            panic!("read");
        };

        assert_eq!(owned(&m), [format!("{UID}_bazel-output")]);
        assert!(
            !fake.argvs().iter().any(|argv| argv[1] == "inspect"),
            "no container, so nothing to inspect: {:?}",
            fake.argvs()
        );
    }

    /// devpod made the record again under a new uid, and the old project's
    /// volume carries the old one. A uid devlaunch kept from an earlier `up`
    /// still names it.
    #[test]
    fn a_compose_volume_of_a_former_uid_is_the_workspaces_own() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        let former = "default-ki-0ld11";
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(bazel_output_rows(UID) + &bazel_output_rows(former)),
        );

        let Recording::Read(m) = record(&fake, Some(&home), &[former.to_owned()], WS) else {
            panic!("read");
        };

        assert_eq!(
            owned(&m),
            [
                format!("{former}_bazel-output"),
                format!("{UID}_bazel-output")
            ]
        );
        // The former uid's containers are looked for as well.
        assert!(
            fake.argvs().contains(&container_ps(&format!(
                "label=com.docker.compose.project={former}"
            ))),
            "{:?}",
            fake.argvs()
        );
    }

    /// The containers a delete swept as leftovers were another project's. Their
    /// compose volumes are read by that project alone, with no uid on record.
    #[test]
    fn the_compose_volumes_of_swept_projects_are_read_by_project() {
        let fake = FakeRunner::new();
        let swept = "default-ki-0ld11";
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(bazel_output_rows(swept)),
        );

        let Recording::Read(m) =
            record_projects(&fake, WS, &[swept.to_owned()].into_iter().collect())
        else {
            panic!("read");
        };

        assert_eq!(owned(&m), [format!("{swept}_bazel-output")]);
    }

    /// Read the mounts of a workspace opened as the bare id `kinisi`, whose
    /// dev container mounts `kinisi-bazel-cache` and `{UID}_bazel-output`, with
    /// docker's compose labels for its volumes as `volume_ls` gives them.
    fn record_bare_kinisi(volume_ls: String) -> Mounted {
        let home = devpod_home_with(&[("default", "kinisi", None)]);
        std::fs::write(
            home.record("default", "kinisi"),
            serde_json::json!({ "id": "kinisi", "uid": UID }).to_string(),
        )
        .expect("a record");
        write_result(&home, "default", "kinisi");
        let fake = FakeRunner::new();
        fake.script(
            container_ps(&format!("label=dev.containers.id={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            container_ps(&format!("label=com.docker.compose.project={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            ["docker", "inspect"],
            Response::stdout(docker::inspect_output(&[inspected(
                UID,
                ComposeFiles::default(),
                &["kinisi-bazel-cache", &format!("{UID}_bazel-output")],
            )])),
        );
        fake.script(["docker", "volume", "ls"], Response::stdout(volume_ls));
        let Recording::Read(m) = record(&fake, Some(&home), &[], "kinisi") else {
            panic!("read");
        };
        m
    }

    #[test]
    fn a_compose_volume_named_on_purpose_stays_when_the_bare_id_is_a_part_of_it() {
        // `bazel-cache: { name: kinisi-bazel-cache }` in the workspace's own
        // compose file.
        let m = record_bare_kinisi(format!(
            "{UID}_bazel-output\t{UID}\tbazel-output\nkinisi-bazel-cache\t{UID}\tbazel-cache\n"
        ));
        assert_eq!(owned(&m), [format!("{UID}_bazel-output")]);
    }

    #[test]
    fn a_compose_volume_named_on_purpose_by_another_project_stays_too() {
        // Compose keeps the label of the project that made the volume first.
        let m = record_bare_kinisi(format!(
            "{UID}_bazel-output\t{UID}\tbazel-output\nkinisi-bazel-cache\tdefault-ki-0ld11\tbazel-cache\n"
        ));
        assert_eq!(owned(&m), [format!("{UID}_bazel-output")]);
    }

    /// Read the mounts of a local folder opened as the bare id `app`, whose dev
    /// container compose started from `compose_file` (written to a scratch dir)
    /// and mounts `app-cache` and `app-pixi`. Compose labelled neither volume.
    fn record_bare_app(compose_file: Option<&str>) -> Recording {
        match compose_file {
            Some(text) => record_bare_app_with(&[("docker-compose.yml", text)]),
            None => record_bare_app_with(&[]),
        }
    }

    /// [`record_bare_app`], with these files written under the scratch dir by
    /// their relative paths: `docker-compose.yml` is the one compose labelled, and
    /// the scratch dir is the project directory.
    fn record_bare_app_with(files: &[(&str, &str)]) -> Recording {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let path = dir.path().join("docker-compose.yml");
        for (relative, text) in files {
            let file = dir.path().join(relative);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("a dir");
            std::fs::write(&file, text).expect("a compose file");
        }
        let home = devpod_home_with(&[("default", "app", None)]);
        std::fs::write(
            home.record("default", "app"),
            serde_json::json!({ "id": "app", "uid": UID }).to_string(),
        )
        .expect("a record");
        write_result(&home, "default", "app");
        let fake = FakeRunner::new();
        fake.script(
            container_ps(&format!("label=dev.containers.id={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            container_ps(&format!("label=com.docker.compose.project={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            ["docker", "inspect"],
            Response::stdout(docker::inspect_output(&[inspected(
                UID,
                ComposeFiles {
                    working_dir: Some(dir.path().to_owned()),
                    files: vec![path],
                },
                &["app-cache", "app-pixi"],
            )])),
        );
        fake.script(["docker", "volume", "ls"], Response::stdout(""));
        record(&fake, Some(&home), &[], "app")
    }

    #[test]
    fn an_external_compose_volume_stays_when_the_bare_id_is_a_part_of_it() {
        // `cache: { external: true, name: app-cache }`: compose did not make it,
        // so it carries no compose label, and other projects share it.
        let Recording::Read(m) = record_bare_app(Some(
            "services:\n  dev:\n    image: x\n    volumes: [cache:/c]\n\
             volumes:\n  cache:\n    external: true\n    name: app-cache\n",
        )) else {
            panic!("read");
        };
        assert_eq!(owned(&m), ["app-pixi".to_owned()]);
    }

    #[test]
    fn an_external_volume_with_no_name_is_named_by_its_key() {
        let Recording::Read(m) =
            record_bare_app(Some("volumes:\n  app-cache:\n    external: true\n"))
        else {
            panic!("read");
        };
        assert_eq!(owned(&m), ["app-pixi".to_owned()]);
    }

    #[test]
    fn the_legacy_external_name_form_is_read() {
        let Recording::Read(m) = record_bare_app(Some(
            "volumes:\n  cache:\n    external:\n      name: app-cache\n",
        )) else {
            panic!("read");
        };
        assert_eq!(owned(&m), ["app-pixi".to_owned()]);
    }

    #[test]
    fn a_volume_not_marked_external_is_still_decided_by_its_name() {
        let Recording::Read(m) = record_bare_app(Some(
            "volumes:\n  app-cache:\n    external: false\n  other: {}\n",
        )) else {
            panic!("read");
        };
        assert_eq!(owned(&m), ["app-cache".to_owned(), "app-pixi".to_owned()]);
    }

    #[test]
    fn a_compose_file_that_cannot_be_read_removes_nothing() {
        let recording = record_bare_app(None);
        assert!(
            matches!(&recording, Recording::Unread { reason } if reason.contains("could not read compose file")),
            "{recording:?}"
        );
    }

    #[test]
    fn a_compose_file_that_cannot_be_parsed_removes_nothing() {
        let recording = record_bare_app(Some("volumes: [unclosed\n"));
        assert!(
            matches!(&recording, Recording::Unread { reason } if reason.contains("could not parse compose file")),
            "{recording:?}"
        );
    }

    #[test]
    fn an_external_name_spelled_with_a_variable_removes_nothing() {
        let recording = record_bare_app(Some(
            "volumes:\n  cache:\n    external: true\n    name: ${PREFIX}-cache\n",
        ));
        assert!(
            matches!(&recording, Recording::Unread { reason } if reason.contains("${PREFIX}-cache")),
            "{recording:?}"
        );
    }

    #[test]
    fn a_merge_key_that_is_not_a_map_removes_nothing() {
        let recording = record_bare_app(Some("volumes:\n  app-cache:\n    <<: true\n"));
        assert!(
            matches!(&recording, Recording::Unread { reason } if reason.contains("docker-compose.yml")),
            "{recording:?}"
        );
    }

    #[test]
    fn an_external_name_spelled_with_an_unbraced_variable_removes_nothing() {
        let recording = record_bare_app(Some(
            "volumes:\n  cache:\n    external: true\n    name: $PREFIX-cache\n",
        ));
        assert!(
            matches!(&recording, Recording::Unread { reason } if reason.contains("$PREFIX-cache")),
            "{recording:?}"
        );
    }

    #[test]
    fn an_external_flag_that_is_not_a_bool_removes_nothing() {
        for flag in ["\"true\"", "yes", "1", "${EXT:-true}"] {
            let recording = record_bare_app(Some(&format!(
                "volumes:\n  app-cache:\n    external: {flag}\n"
            )));
            assert!(
                matches!(&recording, Recording::Unread { reason }
                    if reason.contains("app-cache") && reason.contains("docker-compose.yml")),
                "{flag}: {recording:?}"
            );
        }
    }

    #[test]
    fn one_projects_compose_file_does_not_unmark_another_projects_external_volume() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let a = dir.path().join("a.yml");
        let b = dir.path().join("b.yml");
        std::fs::write(
            &a,
            "volumes:\n  cache:\n    external: true\n    name: app-cache\n",
        )
        .expect("a");
        std::fs::write(&b, "volumes:\n  cache:\n    external: false\n").expect("b");
        let home = devpod_home_with(&[("default", "app", None)]);
        std::fs::write(
            home.record("default", "app"),
            serde_json::json!({ "id": "app", "uid": UID }).to_string(),
        )
        .expect("a record");
        write_result(&home, "default", "app");
        let fake = FakeRunner::new();
        fake.script(
            container_ps(&format!("label=dev.containers.id={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            container_ps(&format!("label=com.docker.compose.project={UID}")),
            Response::stdout("bbb\n"),
        );
        fake.script(
            ["docker", "inspect"],
            Response::stdout(docker::inspect_output(&[
                inspected(
                    "project-a",
                    ComposeFiles {
                        working_dir: None,
                        files: vec![a],
                    },
                    &["app-cache"],
                ),
                inspected(
                    UID,
                    ComposeFiles {
                        working_dir: None,
                        files: vec![b],
                    },
                    &["app-pixi"],
                ),
            ])),
        );
        fake.script(["docker", "volume", "ls"], Response::stdout(""));

        let Recording::Read(m) = record(&fake, Some(&home), &[], "app") else {
            panic!("read");
        };

        assert_eq!(owned(&m), ["app-pixi".to_owned()]);
    }

    #[test]
    fn an_external_volume_an_included_file_declares_stays() {
        let Recording::Read(m) = record_bare_app_with(&[
            (
                "docker-compose.yml",
                "include:\n  - sub/cache.yml\nservices:\n  dev:\n    image: x\n",
            ),
            (
                "sub/cache.yml",
                "volumes:\n  cache:\n    external: true\n    name: app-cache\n",
            ),
        ]) else {
            panic!("read");
        };
        assert_eq!(owned(&m), ["app-pixi".to_owned()]);
    }

    #[test]
    fn an_external_volume_a_later_yaml_document_declares_stays() {
        // compose merges every document of a file in order, as it merges files.
        let Recording::Read(m) = record_bare_app(Some(
            "services:\n  dev:\n    image: x\n    volumes: [cache:/c]\n\
             ---\nvolumes:\n  cache:\n    external: true\n    name: app-cache\n",
        )) else {
            panic!("read");
        };
        assert_eq!(owned(&m), ["app-pixi".to_owned()]);
    }

    #[test]
    fn an_include_that_cannot_be_read_removes_nothing() {
        let recording = record_bare_app_with(&[("docker-compose.yml", "include: [missing.yml]\n")]);
        assert!(
            matches!(&recording, Recording::Unread { reason } if reason.contains("missing.yml")),
            "{recording:?}"
        );
    }

    #[test]
    fn a_workspace_with_no_record_reads_nothing_and_asks_docker_nothing() {
        let home = devpod_home_with(&[]);
        let fake = FakeRunner::new();

        assert_eq!(record(&fake, Some(&home), &[], WS), Recording::Nothing);
        assert!(fake.argvs().is_empty());
    }

    #[test]
    fn a_machine_with_no_docker_reads_nothing() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(record(&fake, Some(&home), &[], WS), Recording::Nothing);
    }

    #[test]
    fn a_docker_that_will_not_list_is_said() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script(["docker", "ps"], Response::failed(1, "daemon down\n"));

        assert!(matches!(
            record(&fake, Some(&home), &[], WS),
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

        let Recording::Read(m) = record(&fake, Some(&home), &[], WS) else {
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

    /// The devcontainer spec substitutes only `${...}`, so `$PREFIX` and `$$` in a
    /// mount source are literal text, unlike in a compose file. Such a source can
    /// only add to the names a delete keeps. And docker refuses a volume name with
    /// a `$`, `{` or `}` in it, so no volume a container mounts can match a source
    /// this read leaves out for its `${`: the read cannot make a delete remove one.
    #[test]
    fn a_dollar_with_no_brace_in_a_devcontainer_mount_is_literal_text() {
        let result = serde_json::json!({
            "DevContainerConfigWithPath": { "config": { "mounts": [
                { "type": "volume", "source": "$PREFIX-cache", "target": "/x" },
                "source=a$$b,target=/y,type=volume",
                { "type": "volume", "source": "${localEnv:TEAM}-cache", "target": "/z" },
            ]}},
        });
        assert_eq!(
            literal_volumes(&result),
            ["$PREFIX-cache", "a$$b"]
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
    fn a_volume_docker_will_not_say_who_uses_is_kept_and_reported() {
        let m = mounted(&[&format!("{WS}-a"), &format!("{WS}-b")]);
        let fake = FakeRunner::new();
        fake.script(
            [
                "docker",
                "ps",
                "--all",
                "--quiet",
                "--no-trunc",
                "--filter",
                &format!("volume={WS}-a"),
            ],
            Response::failed(1, "daemon down\n"),
        );

        let swept = sweep(&fake, &m);

        assert!(
            matches!(&swept[0], Swept::NotRemoved { name, reason }
                if *name == format!("{WS}-a") && reason.contains("docker ps failed") && reason.contains("daemon down")),
            "{swept:?}"
        );
        assert_eq!(swept[1], Swept::Removed(format!("{WS}-b")));
        let removals: Vec<_> = fake
            .argvs()
            .into_iter()
            .filter(|argv| argv[1] == "volume")
            .collect();
        assert_eq!(removals, [["docker", "volume", "rm", &format!("{WS}-b")]]);
    }

    #[test]
    fn a_volume_rm_that_does_not_finish_is_reported_and_the_rest_still_go() {
        let m = mounted(&[&format!("{WS}-a"), &format!("{WS}-b")]);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "rm", &format!("{WS}-a")],
            Response::TimedOut,
        );

        let swept = sweep(&fake, &m);

        assert!(
            matches!(&swept[0], Swept::NotRemoved { name, reason }
                if *name == format!("{WS}-a") && reason.contains("did not finish")),
            "{swept:?}"
        );
        assert_eq!(swept[1], Swept::Removed(format!("{WS}-b")));
    }

    const FORMER: &str = "default-ki-0ld11";

    /// A workspace whose record carries UID and whose current config was read,
    /// and one container of the FORMER uid's project that mounts `volumes`.
    fn record_former_container(volumes: &[&str], home: &DevpodHome) -> Mounted {
        let fake = FakeRunner::new();
        fake.script(
            container_ps(&format!("label=dev.containers.id={FORMER}")),
            Response::stdout("fff\n"),
        );
        fake.script(
            ["docker", "inspect"],
            Response::stdout(docker::inspect_output(&[inspected(
                FORMER,
                ComposeFiles::default(),
                volumes,
            )])),
        );
        let Recording::Read(m) = record(&fake, Some(home), &[FORMER.to_owned()], WS) else {
            panic!("read");
        };
        m
    }

    /// A container of an older uid mounts a volume its own, older config spelled
    /// literally. The current config no longer names it, so the current config
    /// cannot say it is literal, and the id inside its name proves nothing:
    /// it stays.
    #[test]
    fn a_volume_only_a_former_uid_mounts_is_not_claimed_by_the_id() {
        let home = home_with_result();
        let shared = format!("{WS}-shared");

        let m = record_former_container(&[&shared], &home);

        assert!(m.volumes.contains(&shared), "{m:?}");
        assert_eq!(owned(&m), Vec::<String>::new());
    }

    /// The docker-in-docker volume of an older uid holds that uid, a value devpod
    /// made for this workspace alone, so it goes with the workspace.
    #[test]
    fn a_volume_named_after_a_former_uid_is_the_workspaces_own() {
        let home = home_with_result();
        // The docker-in-docker volume of FORMER.
        let dind = "dind-var-lib-docker-default-ki-0ld11";

        let m = record_former_container(&[dind], &home);

        assert_eq!(owned(&m), [dind.to_owned()]);
    }

    /// With no create result, dl cannot read which names the config spells
    /// literally, so a name that holds the id may be a shared one: it stays.
    /// A name that holds devpod's uid is still the workspace's.
    #[test]
    fn with_no_config_to_read_the_id_claims_nothing() {
        let home = home_with_uid();
        let fake = FakeRunner::new();
        fake.script(
            container_ps(&format!("label=dev.containers.id={UID}")),
            Response::stdout("aaa\n"),
        );
        fake.script(
            ["docker", "inspect"],
            Response::stdout(docker::inspect_output(&[inspected(
                UID,
                ComposeFiles::default(),
                &[&format!("{WS}-pixi"), DIND],
            )])),
        );

        let Recording::Read(m) = record(&fake, Some(&home), &[], WS) else {
            panic!("read");
        };

        assert_eq!(owned(&m), [DIND.to_owned()]);
    }

    /// The same id in two devpod contexts: the uids devlaunch kept may be the
    /// other context's workspace, so they are not read at all.
    #[test]
    fn kept_uids_are_not_read_when_two_contexts_hold_the_id() {
        let home = devpod_home_with(&[("default", WS, None), ("other", WS, None)]);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(bazel_output_rows(FORMER)),
        );

        let recording = record(&fake, Some(&home), &[FORMER.to_owned()], WS);

        assert_eq!(recording, Recording::Nothing);
        assert!(fake.argvs().is_empty(), "{:?}", fake.argvs());
    }

    /// With no record in any context, the kept uids are the workspace's alone.
    #[test]
    fn kept_uids_are_read_when_no_context_holds_the_id() {
        let home = devpod_home_with(&[]);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(bazel_output_rows(FORMER)),
        );

        let Recording::Read(m) = record(&fake, Some(&home), &[FORMER.to_owned()], WS) else {
            panic!("read");
        };

        assert_eq!(owned(&m), [format!("{FORMER}_bazel-output")]);
    }

    #[test]
    fn a_name_is_a_part_only_between_separators() {
        assert!(has_part("ws-pixi", "ws"));
        assert!(has_part("a.ws", "ws"));
        assert!(!has_part("ws2-pixi", "ws"));
        assert!(!has_part("anything", ""));
    }
}
