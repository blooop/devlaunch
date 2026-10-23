//! Everything devlaunch asks `docker`, and everything it reads back.
//!
//! `dl <ws> rm` removes the named volumes a workspace's devcontainer created,
//! once the workspace itself is gone (devlaunch#325). `dl <ws> kill` asks which
//! containers a wedged workspace's compose project still has up, and kills them
//! (devlaunch#484) — the reason [`Running`] exists beside [`Answer`]. `rm` and the
//! launch both list compose containers by their labels and remove the ones a
//! forgotten workspace left behind ([`compose_containers`], [`remove_containers`];
//! see [`crate::flows::orphan_containers`]).
//!
//! Nothing here removes an *image* — images are shared, they are expensive to
//! rebuild, and which workspace owns one is genuinely ambiguous, so they stay
//! outside the line `dl --purge` and `dl --prune` both print.
//!
//! # Why this is a module and not three lines in the flow
//!
//! For the reason [`crate::clients::devpod`] is one: **exactly one place may
//! decide that docker is absent.** A host with no docker never created these
//! volumes, so "not installed" is the one answer that must be silent — and folded
//! into an exit status it would read as a docker that ran and refused, which is a
//! sentence about a machine that has nothing to remove. [`Answer`] keeps the two
//! apart, and these are the only spawns that can produce either.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::clients::compose_files::ComposeFiles;
use crate::domain::workspace_state::NonEmpty;
use crate::runner::{CapturedText, Exit, Invocation, OsFailure, Outcome, Runner, SpawnSpec};

/// The program every call in this module runs. One constant, so no caller spells
/// it and a test's response table has one name to match on.
pub(crate) const PROGRAM: &str = "docker";

/// What asking one compose project for its running containers may cost before it
/// is abandoned.
///
/// Bounded, unlike the `docker volume rm` above it, and for [`super::ps`]'s
/// reason rather than for tidiness. `dl <ws> kill` reaches both of the calls
/// below *after* it has already SIGKILLed something, and nothing it found is
/// printed until they return: a docker daemon that never answers would mean the
/// hammer landed, the marker went, and the person who typed the verb saw not one
/// line of it (devlaunch#484, "print what it killed"). A bound turns that into a
/// report that says docker would not answer, which is the whole difference.
const LIST_ONE_PROJECT: Duration = Duration::from_secs(5);

/// What killing them may cost. Longer than the listing, because this is a signal
/// per container and the daemon does the work, where the listing is a read.
const KILL_THEM: Duration = Duration::from_secs(10);

/// What asking docker something came to.
///
/// Three arms rather than an exit status, because two of them are not one, and
/// they are not the same absence:
///
/// - [`Answer::NotInstalled`] is a machine with no docker on it. Nothing to
///   report: a machine with no docker made no volumes.
/// - [`Answer::NotStarted`] is a docker that is there and did not answer — the OS
///   refused the spawn, or the child was killed. That is a fact about this
///   machine, and it carries the errno for the binary to phrase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// docker ran to completion, having written this to stderr. stdout is
    /// dropped: `docker volume rm` echoes the names back, which nothing reads.
    Ran { exit: Exit, stderr: String },
    /// docker is not on PATH — or is there and cannot be exec'd, which arrives as
    /// the same ENOENT and points at the same fix.
    NotInstalled,
    /// docker never answered. `failure.kind` is
    /// [`ErrorKind::TimedOut`] where the child was killed, which is the one case
    /// carrying no errno.
    NotStarted(OsFailure),
}

/// Remove these named volumes, counting one that is already gone as removed.
///
/// **One call carrying every name, and `--force` is what makes that safe.**
/// Measured against docker 29.7: `--force` makes an absent volume exit 0, so a
/// repository whose devcontainer never declared one of these names is a silent
/// no-op rather than a complaint on every delete — while a volume some container
/// still holds is *still* refused, which is the one refusal worth reporting. A
/// name-at-a-time loop would buy nothing but round trips and a partial-failure
/// state to model.
///
/// Captured rather than inherited: a successful removal has nothing to say to a
/// user's terminal, and a refusal is a notice this process frames.
pub(crate) fn remove_volumes(runner: &dyn Runner, names: &NonEmpty<String>) -> Answer {
    let mut args = vec!["volume".to_owned(), "rm".to_owned(), "--force".to_owned()];
    args.extend(names.iter().cloned());
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args));
    answer(runner.capture(&spec))
}

/// Remove one tag, and the image under it only when nothing uses that image.
///
/// `--force` because the container that runs the image is still there when this
/// is asked: without it docker refuses the tag of an image any container uses.
/// With it, and a tag rather than an id, docker removes the tag and keeps the
/// image for the container. Callers pass only a tag dl knows devpod made.
pub(crate) fn untag_image(runner: &dyn Runner, reference: &str) -> Answer {
    let spec =
        SpawnSpec::from(Invocation::new(PROGRAM).with_args(["image", "rm", "--force", reference]));
    answer(runner.capture(&spec))
}

/// The containers one compose project has running, or why docker could not say.
///
/// Its own type rather than an [`Answer`] carrying stdout, because this is the
/// one call in the module whose *output* is the answer: everything else here is
/// a removal, and `Answer` drops stdout on purpose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    /// The ids docker listed, in its order. Empty is a project with nothing up,
    /// which is the ordinary case rather than a failure.
    These(Vec<String>),
    /// No docker on PATH, or one that cannot be exec'd.
    NotInstalled,
    /// docker ran and would not answer, having written this to stderr.
    Refused { exit: Exit, stderr: String },
    /// docker never answered.
    NotStarted(OsFailure),
}

/// The containers docker has running for one compose project.
///
/// **The project name is the workspace id**, because devpod names the compose
/// project itself and derives it from the id — it does not honour a
/// `COMPOSE_PROJECT_NAME` from the project's own `.env`. That is devpod's
/// convention rather than devlaunch's, and `docs/devcontainer-projects.md`
/// carries the rest of what follows from it.
///
/// **The honest limit**: a single-container devcontainer is in no compose project
/// at all, so this finds nothing for one. That is not a hole in the sweep so much
/// as a statement about what the sweep is for — the container is not what blocks
/// a launch, the flock is, and killing the process that holds it is what unwedges
/// the workspace either way.
pub(crate) fn running_for_project(runner: &dyn Runner, project: &str) -> Running {
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args([
        "ps",
        "--quiet",
        "--no-trunc",
        "--filter",
        &format!("label=com.docker.compose.project={project}"),
    ]))
    .with_timeout(LIST_ONE_PROJECT);
    match ran(runner.capture(&spec)) {
        Ok((exit, CapturedText { stdout, stderr })) => {
            if exit.is_success() {
                Running::These(
                    stdout
                        .lines()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                        .collect(),
                )
            } else {
                Running::Refused { exit, stderr }
            }
        }
        Err(NoAnswer::NotInstalled) => Running::NotInstalled,
        Err(NoAnswer::NotStarted(failure)) => Running::NotStarted(failure),
    }
}

/// Kill these containers now, in one call.
///
/// `kill` rather than `stop`, and that is the verb the whole command is named
/// for: a container whose build was SIGKILLed has nothing left to shut down in an
/// orderly way, and `stop` would spend its ten-second grace period on each one
/// before sending the same signal.
pub(crate) fn kill_containers(runner: &dyn Runner, ids: &NonEmpty<String>) -> Answer {
    let mut args = vec!["kill".to_owned()];
    args.extend(ids.iter().cloned());
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args)).with_timeout(KILL_THEM);
    answer(runner.capture(&spec))
}

/// What listing every compose container on the machine may cost.
const LIST_COMPOSE_CONTAINERS: Duration = Duration::from_secs(10);

/// What force-removing a handful of containers may cost. A `kill` and an unlink
/// per container, done by the daemon; bounded because the launch path calls it
/// in front of a `devpod up`, and a daemon that never answers must not turn into
/// a launch that never starts.
const REMOVE_THEM: Duration = Duration::from_secs(60);

/// One container docker made for a compose project, and the paths its compose
/// labels name.
///
/// The paths are what devpod's own delete does *not* look at: devpod finds a
/// workspace's container by `dev.containers.id=<uid>`, and a workspace recreated
/// under a new uid no longer matches the containers the old one left. The
/// compose labels still point at the clone and at devpod's per-workspace
/// directory, whatever uid made them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposeContainer {
    /// The full container id.
    pub(crate) id: String,
    /// The container's name, for the line that says it went.
    pub(crate) name: String,
    /// `com.docker.compose.project.working_dir`, when it is set.
    pub(crate) working_dir: Option<PathBuf>,
    /// `com.docker.compose.project.config_files`, split on its commas.
    pub(crate) config_files: Vec<PathBuf>,
}

impl ComposeContainer {
    /// Every path the compose labels name.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.working_dir
            .as_deref()
            .into_iter()
            .chain(self.config_files.iter().map(PathBuf::as_path))
    }
}

/// Why docker could not list its compose containers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unlisted {
    /// No docker on PATH: a machine with no docker made no containers.
    NotInstalled,
    /// docker was there and did not answer, in these words.
    Failed(String),
}

/// The `--format` template [`compose_containers`] reads back: one tab-separated
/// line per container.
const COMPOSE_FORMAT: &str = "{{.ID}}\t{{.Names}}\t\
{{.Label \"com.docker.compose.project.working_dir\"}}\t\
{{.Label \"com.docker.compose.project.config_files\"}}";

/// Every container on this machine that a compose project made, running or not.
pub(crate) fn compose_containers(runner: &dyn Runner) -> Result<Vec<ComposeContainer>, Unlisted> {
    let stdout = listing(
        runner,
        [
            "ps",
            "--all",
            "--no-trunc",
            "--filter",
            "label=com.docker.compose.project",
            "--format",
            COMPOSE_FORMAT,
        ]
        .map(str::to_owned),
    )?;
    Ok(stdout.lines().filter_map(compose_container).collect())
}

/// Which of these containers docker still has, running or not, by full id.
///
/// For after a removal that failed: `docker rm` exits non-zero when any one of
/// several is already gone, and still removes the rest.
pub(crate) fn present_containers(
    runner: &dyn Runner,
    ids: &NonEmpty<String>,
) -> Result<Vec<String>, Unlisted> {
    let mut args = ["ps", "--quiet", "--all", "--no-trunc"]
        .map(str::to_owned)
        .to_vec();
    for id in ids.iter() {
        args.push("--filter".to_owned());
        args.push(format!("id={id}"));
    }
    let stdout = listing(runner, args)?;
    Ok(stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect())
}

/// Full ids of every container, running or not, that carries `label`
/// (`key=value`).
pub(crate) fn containers_labelled(
    runner: &dyn Runner,
    label: &str,
) -> Result<Vec<String>, Unlisted> {
    containers_matching(runner, &format!("label={label}"))
}

/// Full ids of every container, running or not, that one `docker ps --filter`
/// matches.
fn containers_matching(runner: &dyn Runner, filter: &str) -> Result<Vec<String>, Unlisted> {
    ids_from(listing(
        runner,
        ["ps", "--all", "--quiet", "--no-trunc", "--filter", filter].map(str::to_owned),
    )?)
}

/// Full ids of every container, running or not, that mounts the volume `name`.
pub(crate) fn containers_using_volume(
    runner: &dyn Runner,
    name: &str,
) -> Result<Vec<String>, Unlisted> {
    containers_matching(runner, &format!("volume={name}"))
}

fn ids_from(stdout: String) -> Result<Vec<String>, Unlisted> {
    Ok(stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect())
}

/// The compose project one container is in, the compose files it came from, and
/// the named volumes it mounts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContainerMounts {
    /// `com.docker.compose.project`, when the container has one.
    pub(crate) project: Option<String>,
    /// The compose files compose started the container from, and its project
    /// directory. Empty for a container compose did not start.
    pub(crate) compose: ComposeFiles,
    /// The names of the mounts of `Type` `volume`. Binds are not named here.
    pub(crate) volumes: Vec<String>,
}

/// The `--format` [`container_mounts`] reads back: the compose project, the
/// compose files (comma-separated, as compose writes the label), the project
/// directory, then each named
/// volume, tab-separated, one line per container. docker prints `<no value>` for
/// a label the container does not carry.
const MOUNTS_FORMAT: &str = "{{index .Config.Labels \"com.docker.compose.project\"}}\t\
{{index .Config.Labels \"com.docker.compose.project.config_files\"}}\t\
{{index .Config.Labels \"com.docker.compose.project.working_dir\"}}\t\
{{range .Mounts}}{{if eq .Type \"volume\"}}{{.Name}}\t{{end}}{{end}}";

/// What these containers mount, in one `docker inspect`.
pub(crate) fn container_mounts(
    runner: &dyn Runner,
    ids: &NonEmpty<String>,
) -> Result<Vec<ContainerMounts>, Unlisted> {
    let mut args = ["inspect", "--type", "container", "--format", MOUNTS_FORMAT]
        .map(str::to_owned)
        .to_vec();
    args.extend(ids.iter().cloned());
    let stdout = listing(runner, args)?;
    Ok(stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let label = |field: Option<&str>| {
                field
                    .map(str::trim)
                    .filter(|p| !p.is_empty() && *p != "<no value>")
                    .map(str::to_owned)
            };
            let project = label(fields.next());
            let config_files = label(fields.next())
                .map(|files| {
                    files
                        .split(',')
                        .filter(|f| !f.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default();
            let working_dir = label(fields.next()).map(PathBuf::from);
            let volumes = fields
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect();
            ContainerMounts {
                project,
                compose: ComposeFiles {
                    working_dir,
                    files: config_files,
                },
                volumes,
            }
        })
        .collect())
}

/// What docker prints for these containers under [`MOUNTS_FORMAT`], one line
/// each, so a test can script `docker inspect` from the values it means.
#[cfg(test)]
pub(crate) fn inspect_output(containers: &[ContainerMounts]) -> String {
    const NONE: &str = "<no value>";
    containers
        .iter()
        .map(|container| {
            let files = container
                .compose
                .files
                .iter()
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>()
                .join(",");
            let working_dir = container
                .compose
                .working_dir
                .as_ref()
                .map(|dir| dir.display().to_string());
            let volumes: Vec<&str> = container.volumes.iter().map(String::as_str).collect();
            tests::render_inspect(
                MOUNTS_FORMAT,
                &[
                    (
                        "com.docker.compose.project",
                        container.project.as_deref().unwrap_or(NONE),
                    ),
                    (
                        "com.docker.compose.project.config_files",
                        if files.is_empty() { NONE } else { &files },
                    ),
                    (
                        "com.docker.compose.project.working_dir",
                        working_dir.as_deref().unwrap_or(NONE),
                    ),
                ],
                &volumes,
            )
        })
        .collect()
}

/// One volume compose made: its name, the project that made it, and compose's
/// own name for it — `com.docker.compose.volume`, the key under the file's
/// `volumes:`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposeVolume {
    pub(crate) name: String,
    pub(crate) project: String,
    pub(crate) key: String,
}

/// Every volume compose made, for any project.
///
/// Compose labels a volume with the project that made it first, also when the
/// file gives it an explicit `name:` and a later project uses it too.
pub(crate) fn compose_volumes(runner: &dyn Runner) -> Result<Vec<ComposeVolume>, Unlisted> {
    let stdout = listing(
        runner,
        [
            "volume",
            "ls",
            "--filter",
            "label=com.docker.compose.project",
            "--format",
            "{{.Name}}\t{{.Label \"com.docker.compose.project\"}}\t{{.Label \"com.docker.compose.volume\"}}",
        ]
        .map(str::to_owned),
    )?;
    Ok(stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t').map(str::trim);
            let (name, project, key) = (fields.next()?, fields.next()?, fields.next()?);
            Some(ComposeVolume {
                name: name.to_owned(),
                project: project.to_owned(),
                key: key.to_owned(),
            })
        })
        .filter(|volume| !volume.name.is_empty())
        .collect())
}

/// What one `docker volume rm` came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VolumeRemoval {
    /// docker removed it.
    Removed,
    /// docker had no volume of that name.
    Gone,
    /// docker would not remove it, in these words.
    Refused(String),
}

/// Remove one volume, without `--force`, so the answer says whether it was there.
pub(crate) fn remove_volume(runner: &dyn Runner, name: &str) -> VolumeRemoval {
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(["volume", "rm", name]))
        .with_timeout(REMOVE_THEM);
    match ran(runner.capture(&spec)) {
        Ok((exit, _)) if exit.is_success() => VolumeRemoval::Removed,
        Ok((_, CapturedText { stderr, .. })) if stderr.contains("no such volume") => {
            VolumeRemoval::Gone
        }
        Ok((exit, CapturedText { stderr, .. })) => VolumeRemoval::Refused(format!(
            "docker volume rm failed ({exit:?}): {}",
            stderr.trim()
        )),
        Err(NoAnswer::NotInstalled) => VolumeRemoval::Refused("docker is not installed".to_owned()),
        Err(NoAnswer::NotStarted(failure)) => VolumeRemoval::Refused(format!(
            "docker volume rm did not finish ({:?})",
            failure.kind
        )),
    }
}

/// One `docker ps`, its stdout, or why docker would not give it.
fn listing(
    runner: &dyn Runner,
    args: impl IntoIterator<Item = String>,
) -> Result<String, Unlisted> {
    let args: Vec<String> = args.into_iter().collect();
    // The verb the words name: `ps`, `inspect`, or `volume ls`.
    let verb = match args.as_slice() {
        [first, second, ..] if first == "volume" => format!("{first} {second}"),
        [first, ..] => first.clone(),
        [] => String::new(),
    };
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args))
        .with_timeout(LIST_COMPOSE_CONTAINERS);
    match ran(runner.capture(&spec)) {
        Ok((exit, CapturedText { stdout, .. })) if exit.is_success() => Ok(stdout),
        Ok((exit, CapturedText { stderr, .. })) => Err(Unlisted::Failed(format!(
            "docker {verb} failed ({exit:?}): {}",
            stderr.trim()
        ))),
        Err(NoAnswer::NotInstalled) => Err(Unlisted::NotInstalled),
        Err(NoAnswer::NotStarted(failure)) if failure.kind == ErrorKind::TimedOut => Err(
            Unlisted::Failed(format!("docker {verb} did not answer in time")),
        ),
        Err(NoAnswer::NotStarted(failure)) => Err(Unlisted::Failed(format!(
            "docker {verb} could not start ({:?})",
            failure.kind
        ))),
    }
}

/// One line of [`COMPOSE_FORMAT`], or nothing for a line that is not one.
fn compose_container(line: &str) -> Option<ComposeContainer> {
    let mut fields = line.split('\t');
    let id = fields.next()?.trim();
    let name = fields.next()?.trim();
    if id.is_empty() {
        return None;
    }
    let working_dir = fields
        .next()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let config_files = fields
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .map(PathBuf::from)
        .collect();
    Some(ComposeContainer {
        id: id.to_owned(),
        name: name.to_owned(),
        working_dir,
        config_files,
    })
}

/// Remove these containers, running or not, in one call.
///
/// `--force` because a leftover is usually still running: it is the container a
/// delete did not stop.
pub(crate) fn remove_containers(runner: &dyn Runner, ids: &NonEmpty<String>) -> Answer {
    let mut args = vec!["rm".to_owned(), "--force".to_owned()];
    args.extend(ids.iter().cloned());
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args)).with_timeout(REMOVE_THEM);
    answer(runner.capture(&spec))
}

/// What asking docker for every container may cost before it is abandoned.
const LIST_EVERY_CONTAINER: Duration = Duration::from_secs(5);

/// What describing them may cost. A read, like the listing, but one that names
/// every container on the machine at once.
const INSPECT_THEM: Duration = Duration::from_secs(10);

/// What reading one container's process table may cost.
const READ_INSIDE: Duration = Duration::from_secs(10);

/// The ids of every container docker knows, running or not, or why docker
/// could not say.
///
/// The three reads below are for one question: whether the Claude agent that
/// locked a worktree is still running (`flows::agent_worktrees`). That question
/// clears a lock only on an answer, so **every arm that is not an answer is the
/// same `Err`**: no docker on PATH is as much "could not tell" as a docker that
/// refused, because a lock written inside a container this machine cannot see
/// is not proved gone by the machine having no docker.
pub(crate) fn every_container(runner: &dyn Runner) -> Result<Vec<String>, String> {
    let stdout = read(
        runner,
        &["ps", "--all", "--quiet", "--no-trunc"],
        LIST_EVERY_CONTAINER,
    )?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect())
}

/// `docker inspect` over these containers: the JSON array docker prints, whole.
pub(crate) fn inspect(runner: &dyn Runner, ids: &NonEmpty<String>) -> Result<String, String> {
    let mut args = vec!["inspect"];
    args.extend(ids.iter().map(String::as_str));
    read(runner, &args, INSPECT_THEM)
}

/// Run `argv` inside a running container as root, and what it printed.
///
/// As root because a `/proc` mounted with `hidepid` hides other users'
/// processes from everybody else, and a process the reader cannot see would
/// read as a process that is gone.
pub(crate) fn exec_as_root(
    runner: &dyn Runner,
    container: &str,
    argv: &[&str],
) -> Result<String, ExecFailure> {
    let mut args = vec!["exec", "--user", "0", container];
    args.extend_from_slice(argv);
    read_or(runner, &args, READ_INSIDE, |stderr| {
        // docker's words for an exec into a stopped container, stable since
        // docker 1.x: "container <id> is not running".
        stderr
            .contains("is not running")
            .then_some(ExecFailure::NotRunning)
    })
    .map_err(|failure| match failure {
        Refused::Known(failure) => failure,
        Refused::Words(why) => ExecFailure::Other(why),
    })
}

/// Why [`exec_as_root`] printed nothing to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecFailure {
    /// The container is not running, so there is nothing inside it to ask.
    NotRunning,
    /// Any other failure, in words for a person.
    Other(String),
}

impl std::fmt::Display for ExecFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => f.write_str("the container is not running"),
            Self::Other(why) => f.write_str(why),
        }
    }
}

/// A `--format`ted `docker inspect` over these names: what it printed, whatever
/// it exited.
///
/// Whatever it exited, because `docker inspect` over several names prints a line
/// for every name it found and then exits 1 if any one of them was missing. A
/// container removed by hand, or an image reference this machine never pulled,
/// would otherwise cost every other name its answer. So each format the callers
/// pass puts the name's id in the line, and the caller matches on that id and
/// never on the line's position.
///
/// `None` is a docker that did not run at all, which every caller reads as "could
/// not tell".
pub(crate) fn inspect_formatted(
    runner: &dyn Runner,
    kind: &str,
    format: &str,
    names: &NonEmpty<String>,
) -> Option<String> {
    let mut args = vec!["inspect", "--type", kind, "--format", format];
    args.extend(names.iter().map(String::as_str));
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args.iter().copied()))
        .with_timeout(INSPECT_THEM);
    ran(runner.capture(&spec))
        .ok()
        .map(|(_, CapturedText { stdout, .. })| stdout)
}

/// One bounded docker read: its stdout when it exited 0, and otherwise words
/// for why not.
fn read(runner: &dyn Runner, args: &[&str], timeout: Duration) -> Result<String, String> {
    read_or(runner, args, timeout, |_| None::<std::convert::Infallible>).map_err(|failure| {
        match failure {
            Refused::Known(never) => match never {},
            Refused::Words(why) => why,
        }
    })
}

/// Why [`read_or`] has no stdout to give: a refusal the caller knows by its
/// stderr, or words for any other failure.
enum Refused<K> {
    Known(K),
    Words(String),
}

/// [`read`], with `known` asked first about the stderr of a docker that ran and
/// failed, so a refusal the caller tells apart is told apart here, next to the
/// call that printed it.
fn read_or<K>(
    runner: &dyn Runner,
    args: &[&str],
    timeout: Duration,
    known: impl FnOnce(&str) -> Option<K>,
) -> Result<String, Refused<K>> {
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args.iter().copied()))
        .with_timeout(timeout);
    let verb = args.first().copied().unwrap_or_default();
    match ran(runner.capture(&spec)) {
        Ok((exit, CapturedText { stdout, .. })) if exit.is_success() => Ok(stdout),
        Ok((exit, CapturedText { stderr, .. })) => Err(known(&stderr).map_or_else(
            || {
                Refused::Words(format!(
                    "docker {verb} failed ({exit:?}): {}",
                    stderr.trim()
                ))
            },
            Refused::Known,
        )),
        Err(NoAnswer::NotInstalled) => Err(Refused::Words("docker is not installed".to_owned())),
        Err(NoAnswer::NotStarted(failure)) if failure.kind == ErrorKind::TimedOut => Err(
            Refused::Words(format!("docker {verb} did not answer in time")),
        ),
        Err(NoAnswer::NotStarted(failure)) => Err(Refused::Words(format!(
            "docker {verb} could not start ({:?})",
            failure.kind
        ))),
    }
}

/// A docker spawn that produced nothing to read, in the two shapes both answers
/// in this module report it as.
enum NoAnswer {
    NotInstalled,
    NotStarted(OsFailure),
}

/// The one reading of a docker spawn every call in this module shares.
///
/// `Ok` is a docker that ran, whatever it exited: what its status and its streams
/// mean is the caller's, because a removal and a listing read them differently.
/// `Err` is the three ways of not running collapsed to the two answers that
/// differ, and it is here rather than in each caller because the ENOENT arm is
/// the subtle one — two copies of this match are two chances to lose it, which is
/// exactly what [`running_for_project`] had.
fn ran(outcome: Outcome<CapturedText>) -> Result<(Exit, CapturedText), NoAnswer> {
    match outcome {
        Outcome::Ran { exit, io } => Ok((exit, io)),
        Outcome::ProgramNotFound => Err(NoAnswer::NotInstalled),
        // A bound each caller sets for itself, so this arm is reachable: it must
        // not read as a docker that ran, which for a removal would be a removal
        // that never happened.
        Outcome::TimedOut => Err(NoAnswer::NotStarted(OsFailure {
            kind: ErrorKind::TimedOut,
            errno: None,
        })),
        // A docker found on PATH that could not be exec'd fails with ENOENT at
        // exec time, which arrives here rather than as `ProgramNotFound`. It gets
        // the docker-is-missing answer for the same reason devpod's spawn gives it
        // one: the fix is to install a working docker either way.
        Outcome::NotStarted(failure) if failure.kind == ErrorKind::NotFound => {
            Err(NoAnswer::NotInstalled)
        }
        Outcome::NotStarted(failure) => Err(NoAnswer::NotStarted(failure)),
    }
}

/// What a docker spawn whose stdout nothing reads came to.
fn answer(outcome: Outcome<CapturedText>) -> Answer {
    match ran(outcome) {
        Ok((exit, CapturedText { stderr, .. })) => Answer::Ran { exit, stderr },
        Err(NoAnswer::NotInstalled) => Answer::NotInstalled,
        Err(NoAnswer::NotStarted(failure)) => Answer::NotStarted(failure),
    }
}

#[cfg(test)]
mod tests {
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;

    #[test]
    fn a_containers_mounts_carry_its_project_and_compose_files() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "inspect"],
            Response::stdout(
                "p\t/a/compose.yml,/b/override.yml\t/a\tv1\tv2\t\n<no value>\t<no value>\t<no value>\tv3\t\n",
            ),
        );
        let ids = NonEmpty::of(["aaa".to_owned(), "bbb".to_owned()]).expect("two ids");

        let mounts = container_mounts(&fake, &ids).expect("an answer");

        assert_eq!(
            mounts,
            [
                ContainerMounts {
                    project: Some("p".to_owned()),
                    compose: ComposeFiles {
                        working_dir: Some("/a".into()),
                        files: vec!["/a/compose.yml".into(), "/b/override.yml".into()],
                    },
                    volumes: vec!["v1".to_owned(), "v2".to_owned()],
                },
                ContainerMounts {
                    project: None,
                    compose: ComposeFiles {
                        working_dir: None,
                        files: Vec::new(),
                    },
                    volumes: vec!["v3".to_owned()],
                },
            ]
        );
    }

    #[test]
    fn inspect_output_renders_the_line_docker_prints() {
        assert_eq!(
            inspect_output(&[
                ContainerMounts {
                    project: Some("p".to_owned()),
                    compose: ComposeFiles {
                        working_dir: Some("/a".into()),
                        files: vec!["/a/compose.yml".into(), "/b/override.yml".into()],
                    },
                    volumes: vec!["v1".to_owned(), "v2".to_owned()],
                },
                ContainerMounts {
                    project: None,
                    compose: ComposeFiles {
                        working_dir: None,
                        files: Vec::new(),
                    },
                    volumes: vec!["v3".to_owned()],
                },
            ]),
            "p\t/a/compose.yml,/b/override.yml\t/a\tv1\tv2\t\n<no value>\t<no value>\t<no value>\tv3\t\n"
        );
    }

    /// What docker would print for `format` and one container with these labels
    /// and named volumes: each `{{index .Config.Labels "KEY"}}` becomes that
    /// label, and the `{{range .Mounts}}` body repeats once per volume.
    pub(super) fn render_inspect(
        format: &str,
        labels: &[(&str, &str)],
        volumes: &[&str],
    ) -> String {
        let mut out = format.to_owned();
        for (key, value) in labels {
            out = out.replace(&format!("{{{{index .Config.Labels \"{key}\"}}}}"), value);
        }
        let (head, rest) = out
            .split_once("{{range .Mounts}}")
            .expect("the format ranges over the mounts");
        let (body, tail) = rest
            .rsplit_once("{{end}}{{end}}")
            .expect("the mounts range closes");
        let body = body.replace("{{if eq .Type \"volume\"}}", "");
        let mounts: String = volumes
            .iter()
            .map(|name| body.replace("{{.Name}}", name))
            .collect();
        let rendered = format!("{head}{mounts}{tail}\n");
        assert!(
            !rendered.contains("{{"),
            "an action the stand-in does not know: {rendered:?}"
        );
        rendered
    }

    #[test]
    fn the_inspect_format_prints_the_fields_in_the_order_the_parser_reads() {
        let asked = FakeRunner::new();
        asked.script(["docker", "inspect"], Response::stdout(""));
        let ids = NonEmpty::of(["aaa".to_owned()]).expect("one id");
        let _ = container_mounts(&asked, &ids);
        let argv = asked.argvs().pop().expect("one inspect");
        let format = argv
            .iter()
            .skip_while(|arg| *arg != "--format")
            .nth(1)
            .expect("a --format");

        let printed = render_inspect(
            format,
            &[
                ("com.docker.compose.project", "proj"),
                (
                    "com.docker.compose.project.config_files",
                    "/a/compose.yml,/b/override.yml",
                ),
                ("com.docker.compose.project.working_dir", "/a"),
            ],
            &["v1", "v2"],
        );
        let docker = FakeRunner::new();
        docker.script(["docker", "inspect"], Response::stdout(printed));

        assert_eq!(
            container_mounts(&docker, &ids).expect("an answer"),
            [ContainerMounts {
                project: Some("proj".to_owned()),
                compose: ComposeFiles {
                    working_dir: Some("/a".into()),
                    files: vec!["/a/compose.yml".into(), "/b/override.yml".into()],
                },
                volumes: vec!["v1".to_owned(), "v2".to_owned()],
            }]
        );
    }

    #[test]
    fn a_compose_projects_containers_are_asked_for_by_label() {
        let fake = FakeRunner::new();

        let _ = running_for_project(&fake, "my-ws");

        assert_eq!(
            fake.argvs(),
            [[
                "docker",
                "ps",
                "--quiet",
                "--no-trunc",
                "--filter",
                "label=com.docker.compose.project=my-ws",
            ]]
        );
    }

    /// One id per line, and a project with nothing up prints nothing at all --
    /// which is the ordinary case rather than a failure, since the container is
    /// usually the first thing to die.
    #[test]
    fn the_listing_is_one_container_id_per_line() {
        let fake = FakeRunner::new();
        fake.script(["docker", "ps"], Response::stdout("abc123\ndef456\n"));

        assert_eq!(
            running_for_project(&fake, "my-ws"),
            Running::These(vec!["abc123".to_owned(), "def456".to_owned()])
        );

        let empty = FakeRunner::new();
        empty.script(["docker", "ps"], Response::stdout(""));
        assert_eq!(running_for_project(&empty, "my-ws"), Running::These(vec![]));
    }

    #[test]
    fn a_machine_with_no_docker_lists_no_containers_rather_than_none() {
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(running_for_project(&fake, "my-ws"), Running::NotInstalled);
    }

    /// `kill`, not `stop`: a container belonging to a workspace whose build was
    /// SIGKILLed has nothing left to shut down gracefully, and `stop` would spend
    /// its ten-second grace on each one before doing this anyway.
    #[test]
    fn every_container_is_killed_in_one_call() {
        let fake = FakeRunner::new();

        kill_containers(&fake, &names(&["abc123", "def456"]));

        assert_eq!(fake.argvs(), [["docker", "kill", "abc123", "def456"]]);
    }

    /// The two calls `dl <ws> kill` makes of docker, and both carry a deadline.
    /// Nothing the sweep found reaches the terminal until they return, so a
    /// daemon that never answers would swallow the report of a SIGKILL that has
    /// already landed — devlaunch#484's "print what it killed", lost to the one
    /// thing the verb has no bound on.
    #[test]
    fn neither_call_the_kill_makes_can_outlive_a_wedged_daemon() {
        let fake = FakeRunner::new();

        let _ = running_for_project(&fake, "my-ws");
        kill_containers(&fake, &names(&["abc123"]));

        assert_eq!(
            fake.calls()
                .iter()
                .map(|call| call.spec().and_then(|spec| spec.timeout))
                .collect::<Vec<Option<Duration>>>(),
            [Some(LIST_ONE_PROJECT), Some(KILL_THEM)]
        );
    }

    /// The volume sweep is the call that keeps no bound, and that is the contrast
    /// the two above are for: `dl <ws> rm` is not running because the host stopped
    /// answering, and a volume docker is slow to release is a wait worth taking.
    #[test]
    fn the_volume_removal_keeps_no_deadline() {
        let fake = FakeRunner::new();

        remove_volumes(&fake, &names(&["ws-pixi"]));

        assert_eq!(fake.only_call().spec().and_then(|spec| spec.timeout), None);
    }

    fn names(items: &[&str]) -> NonEmpty<String> {
        NonEmpty::of(items.iter().map(|name| (*name).to_owned())).expect("at least one name")
    }

    #[test]
    fn every_name_goes_in_one_forced_removal() {
        let fake = FakeRunner::new();

        remove_volumes(&fake, &names(&["ws-pixi", "dind-var-lib-docker-abc"]));

        assert_eq!(
            fake.argvs(),
            [[
                "docker",
                "volume",
                "rm",
                "--force",
                "ws-pixi",
                "dind-var-lib-docker-abc",
            ]]
        );
    }

    /// docker's own words for an exec into a stopped container, as it prints
    /// them, read as a container that is not running rather than as a fault.
    #[test]
    fn an_exec_into_a_stopped_container_says_it_is_not_running() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "exec"],
            Response::failed(
                1,
                "Error response from daemon: container 4f1c2a9be0d3 is not running\n",
            ),
        );

        assert_eq!(
            exec_as_root(&fake, "4f1c2a9be0d3", &["true"]),
            Err(ExecFailure::NotRunning)
        );
    }

    #[test]
    fn an_exec_docker_refused_otherwise_keeps_its_words() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "exec"],
            Response::failed(1, "Error response from daemon: permission denied\n"),
        );

        let Err(ExecFailure::Other(why)) = exec_as_root(&fake, "c1", &["true"]) else {
            panic!("not an Other");
        };
        assert!(why.contains("permission denied"), "{why}");
    }

    #[test]
    fn a_docker_that_refused_answers_with_its_own_words() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "rm"],
            Response::failed(1, "volume is in use\n"),
        );

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::Ran {
                exit: Exit::Code(1),
                stderr: "volume is in use\n".to_owned(),
            }
        );
    }

    #[test]
    fn a_machine_with_no_docker_says_so_rather_than_failing() {
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::NotInstalled
        );
    }

    /// A docker on PATH that cannot be exec'd reads as a docker that is not there:
    /// the fix is the same, and the alternative is an errno nobody can act on.
    #[test]
    fn a_docker_that_cannot_be_execd_reads_as_no_docker() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker"],
            Response::NotStarted(OsFailure {
                kind: ErrorKind::NotFound,
                errno: Some(2),
            }),
        );

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::NotInstalled
        );
    }

    #[test]
    fn any_other_os_refusal_keeps_its_errno() {
        let fake = FakeRunner::new();
        let failure = OsFailure {
            kind: ErrorKind::PermissionDenied,
            errno: Some(13),
        };
        fake.script(["docker"], Response::NotStarted(failure));

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::NotStarted(failure)
        );
    }
}
