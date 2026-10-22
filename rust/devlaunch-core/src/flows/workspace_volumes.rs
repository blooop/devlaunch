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
//! explicit `name:` (one compose labelled with a name other than its default
//! `<project>_<volume>`) is shared across projects for the same reason, and so is a
//! volume the compose file marks `external: true`: compose did not make it, so it
//! carries no compose label, and it is read off the compose files instead (the
//! ones the containers' `com.docker.compose.project.config_files` label names).
//! A compose file that cannot be read, or an external name spelled with a
//! variable, makes the whole read [`Recording::Unread`]: no volume of that
//! workspace is removed beyond the two templates, because keeping a volume is
//! the mistake that can be undone.
//!
//! # Only the delete paths reach this
//!
//! [`super::lifecycle::workspace_delete`] is the one caller, and it sits behind
//! `rm`, `rme`, `kill` and `--rm`. `stop`, `up`, recreate and `--refresh-stale`
//! never reach it, so the caches a recreate is meant to keep stay put.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use yaml_rust2::{Yaml, YamlLoader};

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
    let mut config_files: Vec<PathBuf> = Vec::new();
    for container in docker::container_mounts(runner, &ids)? {
        projects.extend(container.project);
        for file in container.config_files {
            if !config_files.contains(&file) {
                config_files.push(file);
            }
        }
        mounted.volumes.extend(container.volumes);
    }
    if mounted.volumes.is_empty() {
        return Ok(());
    }
    mounted
        .literal
        .extend(external_volumes(&config_files).map_err(Unlisted::Failed)?);
    for volume in docker::compose_volumes(runner)? {
        if !mounted.volumes.contains(&volume.name) {
            continue;
        }
        if volume.name != format!("{}_{}", volume.project, volume.key) {
            mounted.literal.insert(volume.name);
        } else if projects.contains(&volume.project) {
            mounted.compose_made.insert(volume.name);
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

/// The names of the volumes these compose files mark `external`, read the way
/// compose merges them: file by file in order, a later file's fields over an
/// earlier one's, per key under the top-level `volumes:`.
///
/// An external volume's name is its `name:`, or the legacy `external: { name: }`,
/// or else its key: compose does not prefix an external name with the project.
/// A file that cannot be read or parsed, an `external:` that is not a bool or a
/// map (compose reads `"true"` or `yes` as true), or a name with a `$` variable in
/// it that only compose's environment could expand, is an error, because a volume the
/// read cannot name is one the sweep cannot be sure to spare.
fn external_volumes(files: &[PathBuf]) -> Result<BTreeSet<String>, String> {
    #[derive(Default)]
    struct Declared {
        external: Option<bool>,
        legacy_name: Option<String>,
        name: Option<String>,
    }
    let mut declared: IndexMap<String, Declared> = IndexMap::new();
    for file in files {
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("could not read compose file {}: {error}", file.display()))?;
        let documents = YamlLoader::load_from_str(&text)
            .map_err(|error| format!("could not parse compose file {}: {error}", file.display()))?;
        let Some(document) = documents.first() else {
            continue;
        };
        let document = apply_merge_keys(document)
            .map_err(|error| format!("could not read compose file {}: {error}", file.display()))?;
        let Some(volumes) = document["volumes"].as_hash() else {
            continue;
        };
        for (key, spec) in volumes {
            let Some(key) = yaml_text(key) else {
                continue;
            };
            let entry = declared.entry(key.clone()).or_default();
            if let Some(name) = spec["name"].as_str() {
                entry.name = Some(name.to_owned());
            }
            match &spec["external"] {
                Yaml::Boolean(external) => {
                    entry.external = Some(*external);
                    entry.legacy_name = None;
                }
                external @ Yaml::Hash(_) => {
                    entry.external = Some(true);
                    entry.legacy_name = external["name"].as_str().map(str::to_owned);
                }
                Yaml::BadValue | Yaml::Null => {}
                _ => {
                    return Err(format!(
                        "compose file {} marks volume {key} external with a value that is not a bool",
                        file.display()
                    ));
                }
            }
        }
    }
    let mut names = BTreeSet::new();
    for (key, entry) in declared {
        if entry.external != Some(true) {
            continue;
        }
        let name = entry.name.or(entry.legacy_name).unwrap_or(key);
        let Some(literal) = unescape_dollars(&name) else {
            return Err(format!(
                "the compose files name external volume {name} with a variable"
            ));
        };
        names.insert(literal);
    }
    Ok(names)
}

/// `value` with every YAML merge key (`<<: *anchor`, or `<<: [*a, *b]`) applied the
/// way compose applies it: a map's own keys win, then the merged maps in order.
fn apply_merge_keys(value: &Yaml) -> Result<Yaml, String> {
    match value {
        Yaml::Hash(hash) => {
            let merge_key = Yaml::String("<<".to_owned());
            let mut applied = yaml_rust2::yaml::Hash::new();
            for (key, item) in hash {
                if *key != merge_key {
                    applied.insert(key.clone(), apply_merge_keys(item)?);
                }
            }
            let sources = match hash.get(&merge_key) {
                None => &[][..],
                Some(source @ Yaml::Hash(_)) => std::slice::from_ref(source),
                Some(Yaml::Array(sources)) => sources.as_slice(),
                Some(_) => return Err("a merge key `<<` that is not a map".to_owned()),
            };
            for source in sources {
                let Yaml::Hash(source) = apply_merge_keys(source)? else {
                    return Err("a merge key `<<` that is not a map".to_owned());
                };
                for (key, item) in source {
                    if !applied.contains_key(&key) {
                        applied.insert(key, item);
                    }
                }
            }
            Ok(Yaml::Hash(applied))
        }
        Yaml::Array(items) => items
            .iter()
            .map(apply_merge_keys)
            .collect::<Result<_, _>>()
            .map(Yaml::Array),
        other => Ok(other.clone()),
    }
}

/// `text` with compose's `$$` escape read as a literal `$`, or `None` when any
/// other `$` starts a variable only compose's environment could expand.
fn unescape_dollars(text: &str) -> Option<String> {
    let mut literal = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '$' && chars.next() != Some('$') {
            return None;
        }
        literal.push(c);
    }
    Some(literal)
}

/// A YAML key or scalar as compose reads it: a string, or a number or bool it
/// spells the same way.
fn yaml_text(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(n) => Some(n.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
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
                "{UID}\t<no value>\t{WS}-pixi\t{UID}_bazel-output\t\n{UID}\t<no value>\tteam-cache\t\n"
            )),
        );
        fake.script(
            ["docker", "volume", "ls"],
            Response::stdout(format!(
                "{UID}_bazel-output\t{UID}\tbazel-output\nnamed-cache\t{UID}\tcache\n"
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
            Response::stdout(format!(
                "{UID}\t<no value>\tkinisi-bazel-cache\t{UID}_bazel-output\t\n"
            )),
        );
        fake.script(["docker", "volume", "ls"], Response::stdout(volume_ls));
        let Recording::Read(m) = record(&fake, Some(&home), "kinisi") else {
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
        let dir = tempfile::tempdir().expect("a scratch dir");
        let path = dir.path().join("docker-compose.yml");
        if let Some(text) = compose_file {
            std::fs::write(&path, text).expect("a compose file");
        }
        let home = devpod_home_with(&[("default", "app", None)]);
        std::fs::write(
            home.record("default", "app"),
            serde_json::json!({ "id": "app", "uid": UID }).to_string(),
        )
        .expect("a record");
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
            Response::stdout(format!(
                "{UID}\t{}\tapp-cache\tapp-pixi\t\n",
                path.display()
            )),
        );
        fake.script(["docker", "volume", "ls"], Response::stdout(""));
        record(&fake, Some(&home), "app")
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
    fn a_merge_key_is_applied_before_external_is_read() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let file = dir.path().join("compose.yml");
        std::fs::write(
            &file,
            "x-ext: &ext\n  external: true\nx-named: &named\n  name: app-named\n\
             volumes:\n  app-cache:\n    <<: *ext\n  \
             listed:\n    <<: [*named, *ext]\n  \
             overridden:\n    <<: *ext\n    external: false\n",
        )
        .expect("compose");
        assert_eq!(
            external_volumes(&[file]),
            Ok(["app-cache", "app-named"]
                .map(str::to_owned)
                .into_iter()
                .collect())
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
    fn a_doubled_dollar_in_an_external_name_is_a_literal_dollar() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let file = dir.path().join("compose.yml");
        std::fs::write(
            &file,
            "volumes:\n  cache:\n    external: true\n    name: a$$b\n",
        )
        .expect("compose");
        assert_eq!(
            external_volumes(&[file]),
            Ok(["a$b".to_owned()].into_iter().collect())
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
    fn a_later_compose_file_overrides_an_earlier_one_key_by_key() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let base = dir.path().join("base.yml");
        let over = dir.path().join("over.yml");
        std::fs::write(&base, "volumes:\n  cache:\n    name: app-cache\n  b: {}\n").expect("base");
        std::fs::write(
            &over,
            "volumes:\n  cache:\n    external: true\n  b:\n    external: true\n    name: app-b\n",
        )
        .expect("override");
        assert_eq!(
            external_volumes(&[base.clone(), over.clone()]),
            Ok(["app-b", "app-cache"]
                .map(str::to_owned)
                .into_iter()
                .collect())
        );
        // And the other way round, `external` from the first file stays unless
        // the second says otherwise.
        assert_eq!(
            external_volumes(&[over, base]),
            Ok(["app-b", "app-cache"]
                .map(str::to_owned)
                .into_iter()
                .collect())
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

    #[test]
    fn a_name_is_a_part_only_between_separators() {
        assert!(has_part("ws-pixi", "ws"));
        assert!(has_part("a.ws", "ws"));
        assert!(!has_part("ws2-pixi", "ws"));
        assert!(!has_part("anything", ""));
    }
}
