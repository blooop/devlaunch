//! Which workspaces run an older image than the reference they were made from
//! now names (devlaunch#673).
//!
//! A container keeps the image it was created from. A `docker pull` of the same
//! reference, by hand or by a daily job, moves the reference and leaves every
//! running container where it was; only the next create picks the new image up.
//! Nothing told anybody, so a long-lived workspace drifted days behind and, for a
//! repository that keys a remote build cache on what the image holds, built its
//! whole tree locally.
//!
//! # What "stale" means here
//!
//! devpod writes the facts down on the way out of every completed `up`, in
//! `workspace_result.json`: the container's id (`ContainerDetails.Id`), the image
//! reference the container was created from (`ContainerDetails.Config.Image`),
//! and the reference the devcontainer *declared* (`MergedConfig.image`), where it
//! declared one. Those two references are not always the same image. devpod builds
//! a derived image on top of the declared one when it has features to install or
//! a user id to remap, and creates the container from that.
//!
//! So there are two cases, and docker answers both:
//!
//! - **The container was created from the declared reference itself.** It is
//!   stale when that reference now names an image other than the one the
//!   container runs. The two ids say so.
//! - **The container was created from an image devpod derived.** The derived tag
//!   never moves on a pull, so its id says nothing. The declared reference's
//!   layers do: a derived image starts with every layer of the image it was built
//!   from, so the container is stale when the declared reference's current layers
//!   are no longer the start of the running image's layers **and** the declared
//!   image was created after the running one. Layers that differ only say
//!   "different". The creation time is what says "older": a prebuilt derived image
//!   pulled from a registry can sit on a newer base than this machine's copy of
//!   the declared reference, and a recreate would pull the same prebuild again,
//!   so without the time the note would never clear.
//!
//! A rebuild of the declared image that changes only its configuration (an `ENV`
//! or a `LABEL`) keeps every layer, so a derived container reads as current
//! through it. That is a miss, not a false alarm.
//!
//! A devcontainer that declares no image (a Dockerfile build, or a compose file
//! whose service names it) falls to the first case with the reference the
//! container was created from. A tag devpod derived from a compose service never
//! moves, so that case reads as current. It is a miss, not a false alarm.
//!
//! # Every doubt reads as current
//!
//! No result file, a container docker no longer has, a reference this machine
//! cannot inspect, a docker that will not answer: each leaves the workspace out
//! of the answer. The only thing the answer drives is a suggestion to recreate,
//! and a recreate costs the processes in the container. A workspace this cannot
//! read about is one it has no grounds to ask anybody to recreate.

use std::collections::{BTreeMap, BTreeSet};

use crate::clients::devpod_home::{DevpodHome, sole_workspace_result};
use crate::clients::docker;
use crate::domain::workspace_state::NonEmpty;
use crate::runner::Runner;

/// One workspace whose container runs an older image than its reference names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleImage {
    reference: String,
}

impl StaleImage {
    /// The reference that now names a newer image: the one the devcontainer
    /// declared, or the one the container was created from where it declared
    /// none.
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

/// The stale workspaces among the ones asked about, by workspace id.
///
/// A workspace that is current and a workspace this could not read about are
/// both absent; see the module documentation for why they are one answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaleImages {
    by_workspace: BTreeMap<String, StaleImage>,
}

impl StaleImages {
    /// What is stale about this workspace, if anything is.
    pub fn of(&self, workspace_id: &str) -> Option<&StaleImage> {
        self.by_workspace.get(workspace_id)
    }

    pub fn is_empty(&self) -> bool {
        self.by_workspace.is_empty()
    }

    /// The answer these pairs of workspace id and reference would make, for a
    /// test that needs one without a docker to read it from.
    #[cfg(test)]
    pub(crate) fn of_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            by_workspace: pairs
                .into_iter()
                .map(|(id, reference)| {
                    (
                        id.to_owned(),
                        StaleImage {
                            reference: reference.to_owned(),
                        },
                    )
                })
                .collect(),
        }
    }
}

/// What devpod recorded about one workspace's container.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Created {
    container: String,
    /// `ContainerDetails.Config.Image`.
    created_from: String,
    /// `MergedConfig.image`, or `created_from` where the devcontainer declared
    /// no image.
    reference: String,
}

/// One image as docker describes it: its id, when it was created, and its
/// layers, bottom first.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Image {
    id: String,
    /// docker's `Created`, cut to whole seconds: `YYYY-MM-DDTHH:MM:SS`. docker
    /// writes it in UTC with a varying count of fractional digits, so the cut is
    /// what makes two of them compare in time order as strings.
    created: String,
    layers: Vec<String>,
}

/// The stale workspaces among these.
///
/// Costs one read of each workspace's result file, then at most two batched
/// `docker inspect` calls plus one per distinct reference. One per reference and
/// not one for all of them, because docker's answer for an image carries its id
/// and not the name it was asked by, and two references can spell one image.
/// Machines with many workspaces of one repository share one reference, so the
/// count is small in practice.
pub fn stale_images<'w>(
    runner: &dyn Runner,
    devpod_home: Option<&DevpodHome>,
    workspace_ids: impl IntoIterator<Item = &'w str>,
) -> StaleImages {
    let created: Vec<(&str, Created)> = workspace_ids
        .into_iter()
        .filter_map(|id| Some((id, created(devpod_home, id)?)))
        .collect();
    let Some(containers) = NonEmpty::of(unique(created.iter().map(|(_, c)| &c.container))) else {
        return StaleImages::default();
    };
    let running = running_images(runner, &containers);
    let running_images = images(runner, unique(running.values()));
    let references: BTreeMap<String, Image> = unique(created.iter().map(|(_, c)| &c.reference))
        .into_iter()
        .filter_map(|reference| {
            let image = images(runner, [reference.clone()]).into_values().next()?;
            Some((reference, image))
        })
        .collect();

    let by_workspace = created
        .into_iter()
        .filter_map(|(id, created)| {
            let running = running_images.get(running.get(&created.container)?)?;
            let current = references.get(&created.reference)?;
            is_stale(&created, running, current).then(|| {
                (
                    id.to_owned(),
                    StaleImage {
                        reference: created.reference,
                    },
                )
            })
        })
        .collect();
    StaleImages { by_workspace }
}

/// Whether a container created as `created` and running `running` is behind
/// the image its reference names now, `current`.
fn is_stale(created: &Created, running: &Image, current: &Image) -> bool {
    if running.id == current.id {
        return false;
    }
    if created.created_from == created.reference {
        return true;
    }
    !running.layers.starts_with(&current.layers) && current.created > running.created
}

/// What devpod's result file says about this workspace's container, if there is
/// exactly one result file and it names a container and an image.
fn created(devpod_home: Option<&DevpodHome>, workspace_id: &str) -> Option<Created> {
    let path = sole_workspace_result(devpod_home, workspace_id)?;
    let document: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let details = &document["ContainerDetails"];
    let created_from = text(&details["Config"]["Image"])?;
    Some(Created {
        container: text(&details["Id"])?,
        reference: text(&document["MergedConfig"]["image"]).unwrap_or_else(|| created_from.clone()),
        created_from,
    })
}

fn text(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn unique<'a>(items: impl Iterator<Item = &'a String>) -> BTreeSet<String> {
    items.cloned().collect()
}

/// The image id each of these containers runs, by container id.
fn running_images(runner: &dyn Runner, containers: &NonEmpty<String>) -> BTreeMap<String, String> {
    let Some(printed) =
        docker::inspect_formatted(runner, "container", "{{.Id}} {{.Image}}", containers)
    else {
        return BTreeMap::new();
    };
    printed
        .lines()
        .filter_map(|line| {
            let (container, image) = line.trim().split_once(' ')?;
            Some((container.to_owned(), image.to_owned()))
        })
        .collect()
}

/// Each of these images that docker has, by image id.
fn images(runner: &dyn Runner, names: impl IntoIterator<Item = String>) -> BTreeMap<String, Image> {
    let Some(names) = NonEmpty::of(names) else {
        return BTreeMap::new();
    };
    let Some(printed) = docker::inspect_formatted(
        runner,
        "image",
        "{{.Id}} {{.Created}} {{json .RootFS.Layers}}",
        &names,
    ) else {
        return BTreeMap::new();
    };
    printed
        .lines()
        .filter_map(|line| {
            let (id, rest) = line.trim().split_once(' ')?;
            let (created, layers) = rest.split_once(' ')?;
            let layers: Vec<String> = serde_json::from_str(layers).ok()?;
            Some((
                id.to_owned(),
                Image {
                    id: id.to_owned(),
                    created: created.get(..19)?.to_owned(),
                    layers,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;
    use crate::clients::devpod_home::{ScratchHome, devpod_home_with};

    const CONTAINER_FORMAT: &str = "{{.Id}} {{.Image}}";
    const IMAGE_FORMAT: &str = "{{.Id}} {{.Created}} {{json .RootFS.Layers}}";

    /// A devpod home whose one workspace was created from `created_from`, with
    /// `declared` as the devcontainer's image where it declared one.
    fn home_with(workspace_id: &str, created_from: &str, declared: Option<&str>) -> ScratchHome {
        let home = devpod_home_with(&[("default", workspace_id, Some(()))]);
        record_create(&home, workspace_id, "c1", created_from, declared);
        home
    }

    fn record_create(
        home: &ScratchHome,
        workspace_id: &str,
        container: &str,
        created_from: &str,
        declared: Option<&str>,
    ) {
        let merged = match declared {
            Some(image) => serde_json::json!({ "image": image }),
            None => serde_json::json!({}),
        };
        std::fs::write(
            home.result("default", workspace_id),
            serde_json::json!({
                "ContainerDetails": { "Id": container, "Config": { "Image": created_from } },
                "MergedConfig": merged,
            })
            .to_string(),
        )
        .expect("a create result");
    }

    fn line(id: &str, layers: &[&str]) -> String {
        line_at(id, "2026-10-01T00:00:00.000000000Z", layers)
    }

    fn line_at(id: &str, created: &str, layers: &[&str]) -> String {
        format!(
            "{id} {created} {}\n",
            serde_json::to_string(layers).expect("layers")
        )
    }

    fn inspect_container(fake: &FakeRunner, running: &str) {
        fake.script(
            [
                "docker",
                "inspect",
                "--type",
                "container",
                "--format",
                CONTAINER_FORMAT,
            ],
            Response::stdout(format!("c1 {running}\n")),
        );
    }

    fn inspect_image(fake: &FakeRunner, name: &str, printed: String) {
        fake.script(
            [
                "docker",
                "inspect",
                "--type",
                "image",
                "--format",
                IMAGE_FORMAT,
                name,
            ],
            Response::stdout(printed),
        );
    }

    #[test]
    fn a_container_made_from_the_reference_is_stale_once_the_reference_moves() {
        let home = home_with("ws", "ghcr.io/o/img:latest", Some("ghcr.io/o/img:latest"));
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:old");
        inspect_image(&fake, "sha256:old", line("sha256:old", &["l1"]));
        inspect_image(
            &fake,
            "ghcr.io/o/img:latest",
            line("sha256:new", &["l1", "l2"]),
        );

        let stale = stale_images(&fake, Some(&home), ["ws"]);

        assert_eq!(
            stale.of("ws").map(StaleImage::reference),
            Some("ghcr.io/o/img:latest")
        );
    }

    #[test]
    fn a_container_made_from_the_reference_is_current_while_the_reference_holds() {
        let home = home_with("ws", "ghcr.io/o/img:latest", None);
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:same");
        inspect_image(&fake, "sha256:same", line("sha256:same", &["l1"]));
        inspect_image(&fake, "ghcr.io/o/img:latest", line("sha256:same", &["l1"]));

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
    }

    #[test]
    fn a_derived_image_is_current_while_it_starts_with_the_references_layers() {
        // devpod built `devpod-abc` on top of the declared image, so the two ids
        // always differ and only the layers can say the base is the same.
        let home = home_with("ws", "devpod-abc", Some("ghcr.io/o/img:latest"));
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:derived");
        inspect_image(
            &fake,
            "sha256:derived",
            line("sha256:derived", &["l1", "l2", "f1"]),
        );
        inspect_image(
            &fake,
            "ghcr.io/o/img:latest",
            line("sha256:base", &["l1", "l2"]),
        );

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
    }

    #[test]
    fn a_derived_image_is_stale_once_the_reference_has_layers_it_lacks() {
        let home = home_with("ws", "devpod-abc", Some("ghcr.io/o/img:latest"));
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:derived");
        inspect_image(
            &fake,
            "sha256:derived",
            line("sha256:derived", &["l1", "l2", "f1"]),
        );
        inspect_image(
            &fake,
            "ghcr.io/o/img:latest",
            line_at("sha256:base2", "2026-10-05T09:00:00Z", &["l1", "l3"]),
        );

        let stale = stale_images(&fake, Some(&home), ["ws"]);

        assert_eq!(
            stale.of("ws").map(StaleImage::reference),
            Some("ghcr.io/o/img:latest")
        );
    }

    #[test]
    fn a_derived_image_is_stale_when_the_references_layers_are_not_its_prefix() {
        let home = home_with("ws", "devpod-abc", Some("ghcr.io/o/img:latest"));
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:derived");
        inspect_image(
            &fake,
            "sha256:derived",
            line("sha256:derived", &["l1", "l2", "f1"]),
        );
        inspect_image(
            &fake,
            "ghcr.io/o/img:latest",
            line_at("sha256:base2", "2026-10-05T09:00:00Z", &["l1", "f1"]),
        );

        let stale = stale_images(&fake, Some(&home), ["ws"]);

        assert_eq!(
            stale.of("ws").map(StaleImage::reference),
            Some("ghcr.io/o/img:latest")
        );
    }

    #[test]
    fn a_prebuilt_image_on_a_newer_base_than_the_local_reference_is_current() {
        // The derived image came from a registry, built on a base this machine
        // has not pulled yet. The layers differ, but the running image is the
        // newer one, and a recreate would pull the same prebuild again.
        let home = home_with(
            "ws",
            "ghcr.io/o/img-devcontainer:abc",
            Some("ghcr.io/o/img:latest"),
        );
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:prebuilt");
        inspect_image(
            &fake,
            "sha256:prebuilt",
            line_at(
                "sha256:prebuilt",
                "2026-10-06T08:00:00.5Z",
                &["l1", "l9", "f1"],
            ),
        );
        inspect_image(
            &fake,
            "ghcr.io/o/img:latest",
            line_at("sha256:base", "2026-10-01T00:00:00Z", &["l1", "l2"]),
        );

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
    }

    #[test]
    fn each_workspace_is_judged_by_its_own_container() {
        let home =
            devpod_home_with(&[("default", "ws-a", Some(())), ("default", "ws-b", Some(()))]);
        let reference = "ghcr.io/o/img:latest";
        record_create(&home, "ws-a", "ca", reference, Some(reference));
        record_create(&home, "ws-b", "cb", reference, Some(reference));
        let fake = FakeRunner::new();
        fake.script(
            [
                "docker",
                "inspect",
                "--type",
                "container",
                "--format",
                CONTAINER_FORMAT,
            ],
            Response::stdout("ca sha256:old\ncb sha256:new\n"),
        );
        fake.script(
            [
                "docker",
                "inspect",
                "--type",
                "image",
                "--format",
                IMAGE_FORMAT,
                "sha256:new",
                "sha256:old",
            ],
            Response::stdout(format!(
                "{}{}",
                line("sha256:new", &["l2"]),
                line("sha256:old", &["l1"])
            )),
        );
        inspect_image(&fake, reference, line("sha256:new", &["l2"]));

        let stale = stale_images(&fake, Some(&home), ["ws-a", "ws-b"]);

        assert_eq!(stale.of("ws-a").map(StaleImage::reference), Some(reference));
        assert_eq!(stale.of("ws-b"), None);
        let docker = fake.args_to("docker");
        let containers: Vec<_> = docker
            .iter()
            .filter(|args| args.contains(&"container".to_owned()))
            .collect();
        assert_eq!(containers.len(), 1);
        assert!(containers[0].ends_with(&["ca".to_owned(), "cb".to_owned()]));
        let reference_inspects = docker
            .iter()
            .filter(|args| args.last().map(String::as_str) == Some(reference))
            .count();
        assert_eq!(reference_inspects, 1);
    }

    #[test]
    fn a_reference_docker_cannot_inspect_reads_as_current() {
        // `docker inspect` over a reference nobody pulled prints nothing and
        // exits 1. That is no grounds to ask for a recreate.
        let home = home_with("ws", "ghcr.io/o/img:latest", None);
        let fake = FakeRunner::new();
        inspect_container(&fake, "sha256:old");
        inspect_image(&fake, "sha256:old", line("sha256:old", &["l1"]));
        fake.script(
            [
                "docker",
                "inspect",
                "--type",
                "image",
                "--format",
                IMAGE_FORMAT,
                "ghcr.io/o/img:latest",
            ],
            Response::failed(1, "Error: No such image"),
        );

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
    }

    #[test]
    fn a_container_docker_no_longer_has_reads_as_current() {
        let home = home_with("ws", "ghcr.io/o/img:latest", None);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "inspect", "--type", "container"],
            Response::failed(1, "Error: No such container: c1"),
        );
        inspect_image(&fake, "ghcr.io/o/img:latest", line("sha256:new", &["l1"]));

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
    }

    #[test]
    fn a_missing_container_costs_the_others_nothing() {
        // docker prints a line for every container it found and exits 1 for the
        // one it did not. The line it printed still counts.
        let home = home_with("ws", "ghcr.io/o/img:latest", None);
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "inspect", "--type", "container"],
            Response::failed(1, "Error: No such container: gone").and_stdout("c1 sha256:old\n"),
        );
        inspect_image(&fake, "sha256:old", line("sha256:old", &["l1"]));
        inspect_image(&fake, "ghcr.io/o/img:latest", line("sha256:new", &["l2"]));

        assert!(stale_images(&fake, Some(&home), ["ws"]).of("ws").is_some());
    }

    #[test]
    fn no_docker_reads_as_nothing_stale() {
        let home = home_with("ws", "ghcr.io/o/img:latest", None);
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
    }

    #[test]
    fn a_workspace_with_no_result_asks_docker_nothing() {
        let home = devpod_home_with(&[("default", "ws", None)]);
        let fake = FakeRunner::new();

        assert!(stale_images(&fake, Some(&home), ["ws"]).is_empty());
        assert!(fake.argvs().is_empty());
    }
}
