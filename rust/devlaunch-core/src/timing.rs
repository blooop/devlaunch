//! Env-gated wall-clock timing for dl's subprocess round trips.
//!
//! Set `DEVLAUNCH_TIMING=1` and every dl process ends with one summary on
//! stderr: a `dl-timing: <label> <seconds>s` line per recorded subprocess call,
//! then a `total` for the whole command. Unset (or `0`) records nothing and
//! reports nothing — the hot path must not pay for its own thermometer, so the
//! off state is one atomic load. stderr because stdout is parsed by the
//! completion machinery, and one summary at the end rather than a line per
//! event, so the numbers land after the command's own output.
//!
//! Set `DEVLAUNCH_TIMING=json` instead and the same run reports as one
//! machine-readable document on a single `dl-timing-json:` line, carrying the
//! named stages of [`Stage`] with the finer spans nested inside them. That mode
//! exists for a trend job that wants stage seconds without scraping prose; `=1`
//! stays exactly the human summary it always was, down to the labels.
//!
//! Ported from `devlaunch/timing.py`; see docs/rust-rewrite-plan.md (M5).
//!
//! # Three differences from the Python module, all deliberate
//!
//! - **The stage vocabulary is an enum, not a string.** Python checked a name
//!   against a tuple and raised `ValueError`, at decoration time so a typo
//!   failed at import rather than on the first measured launch. Here the check
//!   is the type: [`Stage::HostPrep`] cannot be misspelled, and there is no
//!   runtime refusal left to test. The same goes for [`AttachShape`], which
//!   Python validated against a tuple of three strings.
//! - **Nothing here prints.** Python's `emit()` wrote to a stream; core renders
//!   no output (#251), so [`emit`] hands back a [`Report`] and the binary writes
//!   [`Report::lines`] to stderr. The lines are built here because their bytes
//!   are the contract a trend job reads, not English anyone will translate.
//! - **The registry is a value with a process-global handle**, rather than a
//!   module global that tests have to reach into. Everything below [`Registry`]
//!   is exercised on an owned registry; the global is the thin veneer `dl`'s
//!   `main` drives, and the only tests that touch it are the ones about it.
//!
//! # What a caller outside this module records
//!
//! A span is normally a drop guard around the call it times — `let _span =
//! timing::span("devpod up");` — but not every measurement can be taken that
//! way. [`crate::domain::locks`] blocks inside a syscall and reports the wait
//! afterwards as `Contention::Queued { waited }`; there is no scope left to
//! wrap, so its caller hands the finished measurement over with [`record`]
//! ("lock wait", the label Python's `locks.py` spanned it under). Both land in
//! the same place: the flat prose list, and the innermost open stage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// The switch: unset or `0` is off, `json` asks for the document, anything else
/// keeps the prose a habit of `DEVLAUNCH_TIMING=true` always got.
pub(crate) const ENV_VAR: &str = "DEVLAUNCH_TIMING";

/// The value that asks for the document instead of the prose.
pub(crate) const JSON_VALUE: &str = "json";

/// The marker every prose line carries, so it cannot be mistaken for the
/// document's line and so a consumer can find it in stderr that also holds
/// devpod's own chatter.
pub(crate) const PROSE_PREFIX: &str = "dl-timing:";

/// The marker the document's one line carries.
pub(crate) const JSON_PREFIX: &str = "dl-timing-json:";

/// The stamp a hand-off writes for dl to read: Unix epoch seconds as a decimal
/// string, which is what `date +%s.%N` prints. Wall clock rather than a
/// monotonic counter because the two ends are different processes, and there is
/// no clock they share whose zero survives an exec.
///
/// Part of the frozen wf API (#251 §7), re-exported from [`crate::api`]: `wf`
/// stamps this for the `dl` it hands off to, so the name is a contract between
/// the two and `pub` for that reason.
pub const HANDOFF_VAR: &str = "DEVLAUNCH_HANDOFF_T0";

/// The stamp's other half, same format: when a prewarm was fired for this
/// workspace, if one was. Absent means nothing was prewarmed.
///
/// Part of the frozen wf API (#251 §7), re-exported from [`crate::api`].
pub const PREWARM_VAR: &str = "DEVLAUNCH_PREWARM_FIRED_AT";

/// Which clock `total` came from, quoted in the prose line and in the document.
///
/// `total` runs from the top of the command, so it is a smaller quantity than
/// the wall time an outside stopwatch (`scripts/bench_launch.py`) reports for
/// the same command: process startup happens before it. The two get quoted side
/// by side, so each line carries its epoch.
///
/// The words are Python's, byte for byte, because they are quoted in the README
/// and in every bench point recorded so far. "interpreter startup" is the one
/// phrase in them that stops being literally true once the Rust binary is the
/// released `dl`; changing it is a one-line change paired with the README, and
/// it belongs to the cutover rather than to the port.
pub(crate) const TOTAL_EPOCH: &str = "in-process, excluding interpreter startup";

/// One owner of launch latency, in the order a launch meets them.
///
/// A vocabulary read from outside this repo (a trend that decomposes a launch,
/// and the wf side of the handoff), so a name here is renamed only
/// deliberately. `Handoff` is the only one nobody in this process runs: it is
/// the gap between whoever handed off to dl and dl starting, measured from
/// [`HANDOFF_VAR`]. The rest bracket real arms of the launch — the host's git
/// work, the devpod round trips that get a container running, lending the tools
/// in, and the last trip into the running command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Handoff,
    HostPrep,
    DevpodUp,
    Tools,
    Attach,
}

impl Stage {
    /// The vocabulary, in the order a launch meets it — Python's `STAGES`.
    ///
    /// Held for the #251 §7 public-API freeze: these are the handoff stage names,
    /// which cross the process boundary. Only this module's tests read them today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const ALL: [Stage; 5] = [
        Stage::Handoff,
        Stage::HostPrep,
        Stage::DevpodUp,
        Stage::Tools,
        Stage::Attach,
    ];

    /// The name the document reports this stage under.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Stage::Handoff => "handoff",
            Stage::HostPrep => "host-prep",
            Stage::DevpodUp => "devpod-up",
            Stage::Tools => "tools",
            Stage::Attach => "attach",
        }
    }
}

/// What a prewarm turned out to be worth, decided from the arm this launch took
/// rather than from anything the firer said.
///
/// A prewarm is fired and forgotten — the firer is gone before the container
/// finishes — so whether it helped is a fact only this process can witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachShape {
    /// The workspace was already up when this launch asked. Nothing was built
    /// and nothing was waited for; the prewarm did the whole job.
    Hit,
    /// The prewarm was still running, so this launch queued behind it and then
    /// found the workspace up. It was spared the build and paid the wait.
    Partial,
    /// This launch ran `devpod up` itself. Whatever the prewarm did, it did not
    /// save this launch from the container lifecycle.
    Miss,
}

impl AttachShape {
    pub(crate) fn name(self) -> &'static str {
        match self {
            AttachShape::Hit => "hit",
            AttachShape::Partial => "partial",
            AttachShape::Miss => "miss",
        }
    }
}

/// A stage's outcome, two-valued *because the third is absence*: a stage that
/// was never reached has no record at all, which is what keeps "ran fine" and
/// "never ran" from collapsing into one 0.000s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ok,
    Failed,
}

impl Outcome {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
        }
    }
}

/// Which of the two shapes a run reports in, settled once at [`begin`] so
/// nothing downstream has to re-read the environment or agree with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// `DEVLAUNCH_TIMING=1`: the human summary, one line per span.
    Prose,
    /// `DEVLAUNCH_TIMING=json`: one machine-readable document on one line.
    Document,
}

impl Mode {
    /// What `raw` asks for, or `None` for off.
    ///
    /// Off is unset, blank, or `0`. Anything else that is not `json` keeps the
    /// prose, so a habit of `DEVLAUNCH_TIMING=true` still gets what it always
    /// got.
    pub(crate) fn requested(raw: Option<&str>) -> Option<Mode> {
        let asked = crate::osext::strip(raw.unwrap_or(""));
        if asked.is_empty() || asked == "0" {
            return None;
        }
        Some(if asked.eq_ignore_ascii_case(JSON_VALUE) {
            Mode::Document
        } else {
            Mode::Prose
        })
    }

    /// What the process environment asks for.
    pub(crate) fn from_env() -> Option<Mode> {
        Mode::requested(crate::osext::env_str(ENV_VAR).as_deref())
    }
}

/// The two stamps the hand-off seam writes, as read.
///
/// Kept rather than folded away: the handoff stage needs one of them and the
/// head start needs both, and a difference cannot be recovered from a
/// difference.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Seam {
    /// When whoever handed off to dl started, in Unix epoch seconds.
    pub(crate) keystroke: Option<f64>,
    /// When a prewarm was fired for this workspace, in Unix epoch seconds.
    pub(crate) prewarm_fired: Option<f64>,
}

impl Seam {
    /// The seam as two raw strings read it. Unset, blank, not a number, or not
    /// a finite one all answer `None`: none of them is a measurement, and a
    /// stand-in derived from one would be a fiction a trend cannot tell from a
    /// reading.
    pub(crate) fn parse(handoff: Option<&str>, prewarm: Option<&str>) -> Seam {
        Seam {
            keystroke: stamp(handoff),
            prewarm_fired: stamp(prewarm),
        }
    }

    /// The seam as the process environment holds it.
    pub(crate) fn from_env() -> Seam {
        Seam::parse(
            crate::osext::env_str(HANDOFF_VAR).as_deref(),
            crate::osext::env_str(PREWARM_VAR).as_deref(),
        )
    }
}

/// The wall-clock instant `raw` holds, or `None` if it holds no instant.
fn stamp(raw: Option<&str>) -> Option<f64> {
    let stamped = crate::osext::strip(raw.unwrap_or(""));
    if stamped.is_empty() {
        return None;
    }
    stamped
        .parse::<f64>()
        .ok()
        .filter(|instant| instant.is_finite())
}

/// Now, on the same clock the seam's stamps are written on.
fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        // Before 1970 on this machine's clock. Not a stamp anything can be
        // measured against, and answering 0.0 keeps this total: every gap
        // computed from it is then negative, which is already "no measurement".
        .unwrap_or(0.0)
}

/// One timed round trip: what it was, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanRecord {
    pub(crate) label: String,
    pub seconds: Duration,
}

/// One owner's arm: how long it held the launch, and what it spawned.
///
/// `seconds` accumulates, because an owner's work is not always one contiguous
/// region — a token fetch is host prep whenever it happens. "Never reached" is
/// not a value this can hold: it is the absence of the record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StageRecord {
    stage: Stage,
    seconds: Duration,
    outcome: Outcome,
    spans: Vec<SpanRecord>,
}

/// A stage currently on the clock, and the instant it last started.
///
/// Two stages can be open at once (`tools` runs inside the launch that
/// `devpod-up` brackets), so the outer one is paused for the duration of the
/// inner one rather than charged for it twice.
#[derive(Debug)]
struct Open {
    stage: usize,
    since: Instant,
}

/// Whether entering a stage put it on the clock, or found the same owner
/// already there.
///
/// A stage already on the clock is not re-entered: the arm is instrumented at
/// several of its own entry points (a clone that fetches, a fetch that takes the
/// lock), and an inner entry there is the same owner's same work, not a second
/// visit to charge for. The guard remembers which of the two happened, because
/// only the entry that opened the stage may close it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    Opened,
    AlreadyOpen,
}

/// One dl process's records: a start instant and the spans since.
///
/// Constructed only when timing is on, so "recording" is this value existing
/// rather than a flag inside it that other fields would have to agree with.
#[derive(Debug)]
pub(crate) struct Registry {
    mode: Mode,
    started: Instant,
    entries: Vec<SpanRecord>,
    /// Insertion-ordered, which is the order the document reports: a reader
    /// meets the stages in the order the launch met them.
    stages: Vec<StageRecord>,
    open: Vec<Open>,
    seam: Seam,
    attach_shape: Option<AttachShape>,
}

impl Registry {
    /// The registry a measured run begins with.
    ///
    /// `now_epoch` is wall-clock seconds since the Unix epoch — dl's own end of
    /// the hand-off gap, read at the same moment `total`'s clock starts, so the
    /// process startup before it lands on the handoff's side of the boundary
    /// rather than being lost between the two.
    ///
    /// A handoff stage needs the keystroke stamp *and* a non-negative gap from
    /// it; a stamp ahead of this clock describes two clocks that disagree, not a
    /// handoff that took negative time.
    pub(crate) fn start(mode: Mode, seam: Seam, now_epoch: f64) -> Self {
        let mut registry = Self {
            mode,
            started: Instant::now(),
            entries: Vec::new(),
            stages: Vec::new(),
            open: Vec::new(),
            seam,
            attach_shape: None,
        };
        if let Some(gap) = seam
            .keystroke
            .and_then(|keystroke| gap(now_epoch - keystroke))
        {
            registry.stages.push(StageRecord {
                stage: Stage::Handoff,
                seconds: gap,
                outcome: Outcome::Ok,
                spans: Vec::new(),
            });
        }
        registry
    }

    /// Record one finished measurement: the flat prose list, and the innermost
    /// open stage if there is one.
    ///
    /// A spawn that failed still took time, so its caller records it on the way
    /// out either way — dropping it would make the parts add up to less than the
    /// total.
    pub(crate) fn record(&mut self, label: impl Into<String>, took: Duration) {
        let record = SpanRecord {
            label: label.into(),
            seconds: took,
        };
        if let Some(open) = self.open.last() {
            self.stages[open.stage].spans.push(record.clone());
        }
        self.entries.push(record);
    }

    /// Put `stage` on the clock, pausing the stage it interrupts.
    ///
    /// Answers [`Entry::AlreadyOpen`] — and changes nothing — when this owner is
    /// already on the clock.
    pub(crate) fn enter(&mut self, stage: Stage) -> Entry {
        if self
            .open
            .iter()
            .any(|open| self.stages[open.stage].stage == stage)
        {
            return Entry::AlreadyOpen;
        }
        let now = Instant::now();
        if let Some(paused) = self.open.last_mut() {
            let held = now.saturating_duration_since(paused.since);
            let index = paused.stage;
            paused.since = now;
            self.stages[index].seconds += held;
        }
        let stage = self.stage_index(stage);
        self.open.push(Open { stage, since: now });
        Entry::Opened
    }

    /// Take `stage` off the clock with `outcome`, resuming whatever it paused.
    ///
    /// An arm that died still held the launch for as long as it did, and the
    /// failure is the reading rather than a reason to drop it — so the seconds
    /// are charged whichever outcome this is, and an outcome only ever moves
    /// from ok to failed.
    ///
    /// Stages nest, so the one being closed is normally the innermost. A guard
    /// dropped out of order closes the stages above it too, at this same
    /// instant: leaving them open would keep charging them to a clock nothing
    /// will ever stop. A stage that is not open at all closes nothing.
    pub(crate) fn leave(&mut self, stage: Stage, outcome: Outcome) {
        let now = Instant::now();
        let Some(depth) = self
            .open
            .iter()
            .rposition(|open| self.stages[open.stage].stage == stage)
        else {
            return;
        };
        while self.open.len() > depth {
            let Some(open) = self.open.pop() else { break };
            let record = &mut self.stages[open.stage];
            record.seconds += now.saturating_duration_since(open.since);
            if record.stage == stage && outcome == Outcome::Failed {
                record.outcome = Outcome::Failed;
            }
        }
        if let Some(resumed) = self.open.last_mut() {
            resumed.since = now;
        }
    }

    /// Record which arm the launch took.
    ///
    /// Observed rather than inferred — dl is the only party that can see whether
    /// it had to run the `up`. What it means for a prewarm is decided at report
    /// time: with no prewarm fired there is no prewarm outcome to report,
    /// however this launch went.
    pub(crate) fn observe_attach(&mut self, shape: AttachShape) {
        self.attach_shape = Some(shape);
    }

    /// The run's report. Consuming, because nothing records after it.
    pub(crate) fn finish(self) -> Report {
        let total = self.started.elapsed();
        match self.mode {
            Mode::Prose => Report::Prose(Prose {
                spans: self.entries,
                total,
            }),
            Mode::Document => Report::Document(Document {
                total: round6(total),
                total_epoch: TOTAL_EPOCH,
                stages: self
                    .stages
                    .iter()
                    .map(|record| StageReport {
                        stage: record.stage.name(),
                        seconds: round6(record.seconds),
                        outcome: record.outcome.name(),
                        spans: record
                            .spans
                            .iter()
                            .map(|span| SpanReport {
                                label: span.label.clone(),
                                seconds: round6(span.seconds),
                            })
                            .collect(),
                    })
                    .collect(),
                prewarm: self.prewarm().as_ref().map(PrewarmReport::from),
            }),
        }
    }

    /// What this launch can say about the prewarm that preceded it.
    ///
    /// Nothing unless a prewarm was actually fired: with no firing stamp there
    /// is no prewarm whose head start or outcome could be reported, and
    /// reporting one anyway would invent the thing being measured. Each fact
    /// then appears only if it is known — a head start needs both stamps, and a
    /// shape needs a launch that reached one of the arms
    /// [`Registry::observe_attach`] marks — which is why "a prewarm with
    /// nothing known about it" has no arm here rather than being an empty one.
    fn prewarm(&self) -> Option<Prewarm> {
        let fired = self.seam.prewarm_fired?;
        let head_start = self
            .seam
            .keystroke
            .map(|keystroke| keystroke - fired)
            .filter(|seconds| *seconds >= 0.0);
        match (head_start, self.attach_shape) {
            (None, None) => None,
            (Some(seconds), None) => Some(Prewarm::HeadStart { seconds }),
            (None, Some(shape)) => Some(Prewarm::Shape { shape }),
            (Some(seconds), Some(shape)) => Some(Prewarm::Both { seconds, shape }),
        }
    }

    /// The record for `stage`, appending one the first time it is asked for.
    fn stage_index(&mut self, stage: Stage) -> usize {
        if let Some(found) = self.stages.iter().position(|record| record.stage == stage) {
            return found;
        }
        self.stages.push(StageRecord {
            stage,
            seconds: Duration::ZERO,
            outcome: Outcome::Ok,
            spans: Vec::new(),
        });
        self.stages.len() - 1
    }
}

/// What is known about the prewarm that preceded this launch — at least one
/// fact, because "a prewarm nothing is known about" is reported as no prewarm.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Prewarm {
    /// How long the prewarm ran before the hand-off, and nothing about how it
    /// turned out: this launch never reached an arm that would say.
    HeadStart {
        seconds: f64,
    },
    /// What the prewarm turned out to be worth, with no keystroke stamp to
    /// measure its head start against.
    Shape {
        shape: AttachShape,
    },
    Both {
        seconds: f64,
        shape: AttachShape,
    },
}

/// A run's report: the bytes the binary writes, and the data behind them.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    Prose(Prose),
    Document(Document),
}

impl Report {
    /// The lines to write, in order — stderr, because stdout is parsed by the
    /// completion machinery.
    pub fn lines(&self) -> Vec<String> {
        match self {
            Report::Prose(prose) => prose.lines(),
            Report::Document(document) => vec![document.line()],
        }
    }

    /// The document, for a caller that wants the numbers rather than the line.
    ///
    /// Only this module's tests read it; the binary renders [`Report::lines`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn document(&self) -> Option<&Document> {
        match self {
            Report::Prose(_) => None,
            Report::Document(document) => Some(document),
        }
    }
}

/// The human summary: one line per round trip, then the total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prose {
    pub spans: Vec<SpanRecord>,
    pub total: Duration,
}

impl Prose {
    fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .spans
            .iter()
            .map(|span| {
                format!(
                    "{PROSE_PREFIX} {} {:.3}s",
                    span.label,
                    span.seconds.as_secs_f64()
                )
            })
            .collect();
        lines.push(format!(
            "{PROSE_PREFIX} total {:.3}s ({TOTAL_EPOCH})",
            self.total.as_secs_f64()
        ));
        lines
    }
}

/// The whole run as one JSON object, on one marked line.
///
/// One line rather than indented JSON: a consumer greps stderr for the marker
/// and parses what follows, which no amount of surrounding output can break.
/// Field order is the order Python's dict was built in, so the two binaries'
/// documents are byte-comparable.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Document {
    pub(crate) total: f64,
    pub(crate) total_epoch: &'static str,
    pub(crate) stages: Vec<StageReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prewarm: Option<PrewarmReport>,
}

impl Document {
    /// The one line this document is written as.
    pub(crate) fn line(&self) -> String {
        // Spelled the way `json.dumps` spells it (`", "` / `": "` separators, and
        // `ensure_ascii` for a span label that carries a non-ASCII path), which is
        // what makes the byte-comparability above true rather than intended:
        // `serde_json::to_string` writes the compact form and Python never does.
        //
        // Serializing a struct of numbers and strings cannot fail; the fallback
        // keeps the marker greppable rather than swallowing the line.
        let json = crate::json::serialize_as_python(self).unwrap_or_else(|| String::from("{}"));
        format!("{JSON_PREFIX} {json}")
    }
}

/// One stage, as the document reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct StageReport {
    pub(crate) stage: &'static str,
    pub(crate) seconds: f64,
    pub(crate) outcome: &'static str,
    pub(crate) spans: Vec<SpanReport>,
}

/// One round trip, as the document reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SpanReport {
    pub(crate) label: String,
    pub(crate) seconds: f64,
}

/// The prewarm's facts, flattened for the wire.
///
/// [`Prewarm`] is where the "at least one fact" invariant lives; this is the
/// re-encoding at the boundary, where two absent fields are representable
/// because JSON has no other way to say a field is missing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PrewarmReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) head_start_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shape: Option<&'static str>,
}

impl From<&Prewarm> for PrewarmReport {
    fn from(prewarm: &Prewarm) -> Self {
        match *prewarm {
            Prewarm::HeadStart { seconds } => PrewarmReport {
                head_start_seconds: Some(round(seconds)),
                shape: None,
            },
            Prewarm::Shape { shape } => PrewarmReport {
                head_start_seconds: None,
                shape: Some(shape.name()),
            },
            Prewarm::Both { seconds, shape } => PrewarmReport {
                head_start_seconds: Some(round(seconds)),
                shape: Some(shape.name()),
            },
        }
    }
}

/// A gap in seconds as a duration, or `None` if it is not a measurement.
///
/// Negative is two clocks that disagree. Non-finite, or larger than a `Duration`
/// can hold, is a stamp that parsed as a number without being an instant — and
/// answering `None` keeps every caller total.
fn gap(seconds: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(seconds).ok()
}

/// Seconds to six decimal places, as Python's `round(x, 6)` reports them.
fn round6(took: Duration) -> f64 {
    round(took.as_secs_f64())
}

fn round(seconds: f64) -> f64 {
    (seconds * 1e6).round() / 1e6
}

// --- the process-global handle ---------------------------------------------
//
// A veneer over `Registry` and nothing more: `dl`'s `main` begins and emits, and
// every span and stage in between is charged to whatever registry is installed.
// The flows that record spans are scattered through every layer and threading a
// registry through all of them would put the thermometer in signatures that have
// no other reason to know about it.

/// Whether a registry is installed, readable without taking the lock.
///
/// The off state has to stay free — this is the check a launch nobody is
/// measuring pays for every span — so it is one relaxed load rather than a mutex
/// acquisition. Relaxed is enough: install and emit both happen on the main
/// thread before and after everything they gate.
static RECORDING: AtomicBool = AtomicBool::new(false);

static REGISTRY: OnceLock<Mutex<Option<Registry>>> = OnceLock::new();

fn registry() -> MutexGuard<'static, Option<Registry>> {
    REGISTRY
        .get_or_init(|| Mutex::new(None))
        .lock()
        // A panicking span guard must not poison the summary of the very run
        // that panicked: the data behind the lock is a pile of measurements, and
        // a half-recorded span leaves it inconsistent in no way that matters.
        .unwrap_or_else(PoisonError::into_inner)
}

/// Run `with` against the installed registry, or do nothing if timing is off.
fn with_registry<T>(with: impl FnOnce(&mut Registry) -> T) -> Option<T> {
    if !RECORDING.load(Ordering::Relaxed) {
        return None;
    }
    registry().as_mut().map(with)
}

/// Start recording iff `DEVLAUNCH_TIMING` asks for it.
///
/// Called once at the top of the command, replacing any registry left from an
/// earlier one in the same process, so one command's spans never leak into the
/// next command's summary.
pub fn begin() {
    install(Mode::from_env().map(|mode| Registry::start(mode, Seam::from_env(), now_epoch())));
}

/// Install `registry` as the process's, replacing whatever was there.
///
/// What [`begin`] does once the environment has been read, and the seam tests
/// drive to exercise the veneer without touching the environment.
pub(crate) fn install(registry_to_install: Option<Registry>) {
    let installed = registry_to_install.is_some();
    *registry() = registry_to_install;
    RECORDING.store(installed, Ordering::Relaxed);
}

/// The report for this run, and stop recording; `None` if recording never began.
pub fn emit() -> Option<Report> {
    RECORDING.store(false, Ordering::Relaxed);
    registry().take().map(Registry::finish)
}

/// Record a measurement somebody else took, under `label`.
///
/// For the one span that cannot be a guard: a lock acquisition that blocked
/// reports how long it waited only once the wait is over.
pub(crate) fn record(label: impl Into<String>, took: Duration) {
    with_registry(|registry| registry.record(label, took));
}

/// Time one round trip as `label`, until the guard drops.
///
/// ```ignore
/// let _span = timing::span("devpod up");
/// ```
///
/// When timing is off this reads the clock, allocates, and records nothing.
#[must_use = "the round trip is timed until this guard drops, so bind it"]
pub(crate) fn span(label: impl Into<String>) -> SpanGuard {
    if !RECORDING.load(Ordering::Relaxed) {
        return SpanGuard { timing: None };
    }
    SpanGuard {
        timing: Some((label.into(), Instant::now())),
    }
}

/// A round trip on the clock. Records when it drops, however the block ended.
#[must_use = "the round trip is timed until this guard drops, so bind it"]
pub(crate) struct SpanGuard {
    timing: Option<(String, Instant)>,
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if let Some((label, started)) = self.timing.take() {
            record(label, started.elapsed());
        }
    }
}

/// Charge everything until the guard drops to the ownership-boundary `stage`.
///
/// The stage is reported failed if the guard drops during a panic, or if the
/// caller says so with [`StageGuard::fail`]. A `?` that returns early is neither
/// of those, which is why fallible arms are better spelled [`stage_result`].
#[must_use = "the stage is on the clock until this guard drops, so bind it"]
pub(crate) fn stage(stage: Stage) -> StageGuard {
    let entered = with_registry(|registry| registry.enter(stage));
    StageGuard {
        // Only the entry that opened the stage may close it.
        stage: match entered {
            Some(Entry::Opened) => Some(stage),
            Some(Entry::AlreadyOpen) | None => None,
        },
        outcome: Outcome::Ok,
    }
}

/// Charge `work` to `stage`, reporting the stage failed if `work` fails.
///
/// The port of Python's `@staged` decorator: the launch's arms are mostly whole
/// functions — bring the workspace up, lend the tools in, attach — and most of
/// them answer with a `Result`, where the failure the stage should report is a
/// returned `Err` rather than an unwinding panic.
pub(crate) fn stage_result<T, E>(
    stage_to_charge: Stage,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let mut guard = stage(stage_to_charge);
    let outcome = work();
    if outcome.is_err() {
        guard.fail();
    }
    outcome
}

/// A stage on the clock. Comes off it when this drops.
#[must_use = "the stage is on the clock until this guard drops, so bind it"]
pub(crate) struct StageGuard {
    /// The stage this guard is responsible for closing — `None` when timing is
    /// off, or when the same owner was already on the clock.
    stage: Option<Stage>,
    outcome: Outcome,
}

impl StageGuard {
    /// Report this stage failed: the arm did not complete.
    pub(crate) fn fail(&mut self) {
        self.outcome = Outcome::Failed;
    }
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        if let Some(stage) = self.stage.take() {
            let outcome = if std::thread::panicking() {
                Outcome::Failed
            } else {
                self.outcome
            };
            with_registry(|registry| registry.leave(stage, outcome));
        }
    }
}

/// Record which arm the launch took, for the prewarm report.
pub(crate) fn observe_attach(shape: AttachShape) {
    with_registry(|registry| registry.observe_attach(shape));
}

// --- the test exclusion -----------------------------------------------------

/// Exclusive use of the process-global registry, for the length of the guard.
///
/// `cargo test` runs a crate's tests as threads of *one* process, and there is one
/// registry per process — so without this, a test that installs one and a test that
/// merely opens a stage are writing to the same document. Two things then go wrong,
/// and only the first is visible as a wrong number: a stranger's spans and stages
/// land in the measured document, and two concurrent [`stage`] calls for the same
/// owner have the second find it already open, so its guard closes nothing while
/// the first one's closes the stage out from under it.
///
/// Crate-wide rather than per-module, which is the whole point: the hazard is
/// between modules. It is held by the *fixtures* — `launch`'s `Scene`,
/// `repo_manager`'s `Cache`, `lifecycle`'s `Devpod` — rather than asked for at the
/// top of each test, so a new test cannot forget it, and a test that builds two of
/// them does not deadlock: the exclusion is reentrant per thread, and only the
/// outermost guard holds the lock.
///
/// A test whose *body* spawns threads is covered by this too, because what the
/// exclusion is against is another **test** recording, and no other test can be
/// running while this one holds the guard. The one thing such a worker must not do
/// is ask for a guard of its own: reentrancy is per thread, so a worker would be
/// waiting for its own test to finish. That is why the contention tests' `FakeGit`
/// is not a holder and their `Cache` is.
#[cfg(test)]
pub(crate) fn exclusive() -> Exclusive {
    // The `None` arm is a thread already inside the exclusion; taking the lock
    // again there is a deadlock against itself, and needs nothing, because the
    // outer guard already excludes everybody else.
    let held = (DEPTH.get() == 0).then(|| {
        MEASURING
            .lock()
            // A test that panicked while measuring must not fail every later one:
            // what is behind this lock is `()`, so there is no state to have left
            // inconsistent.
            .unwrap_or_else(PoisonError::into_inner)
    });
    DEPTH.set(DEPTH.get() + 1);
    Exclusive { _held: held }
}

#[cfg(test)]
static MEASURING: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// How many live [`Exclusive`] guards this thread already holds.
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The registry is this test's alone until this is dropped.
#[cfg(test)]
#[derive(Debug)]
#[must_use = "the exclusion lasts until this guard drops, so bind it"]
pub(crate) struct Exclusive {
    /// Held by the outermost guard on this thread, and by no inner one.
    _held: Option<MutexGuard<'static, ()>>,
}

#[cfg(test)]
impl Drop for Exclusive {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
    }
}

#[cfg(test)]
mod tests {
    //! What `test/test_timing.py` pins, on this side of the port.
    //!
    //! Its `TestBenchHarness`, `TestTheDocumentedColdReset`,
    //! `TestBenchRecordsEachRunsStages` and `TestEveryDocumentedInvocation`
    //! classes are about `scripts/bench_launch.py` — a Python script that stays
    //! Python, and whose ledger rows are already `out of port scope` — so they
    //! have no analogue here. Neither does
    //! `TestAMistypedStageNameFailsWhereItIsWritten`: [`Stage`] is an enum, so
    //! the refusal it pins is a compile error rather than a `ValueError`. The
    //! classes that drive a whole launch (`TestDevpodRoundTripsAreNamed`,
    //! `TestAWarmLaunchReportsItsStages`, `TestAColdStartReportsItsStages`,
    //! `TestHostPrepIsAStage`, the two prewarm-shape classes) pin *which* spans
    //! the launch path records, which arrives with the launch path in M7; what
    //! they need from this module — nesting, accumulation, absence, shapes — is
    //! pinned here on the registry itself.

    use super::*;

    /// Long enough that a sleep of it is unmistakable in a duration, short
    /// enough that a suite of them is not the slow part of anything.
    const TICK: Duration = Duration::from_millis(30);

    fn document_of(registry: Registry) -> Document {
        match registry.finish() {
            Report::Document(document) => document,
            Report::Prose(prose) => panic!("asked for a document, got {prose:?}"),
        }
    }

    fn recording() -> Registry {
        Registry::start(Mode::Document, Seam::default(), now_epoch())
    }

    fn stage_names(document: &Document) -> Vec<&str> {
        document.stages.iter().map(|stage| stage.stage).collect()
    }

    fn stage_named<'a>(document: &'a Document, name: &str) -> &'a StageReport {
        let found: Vec<&StageReport> = document
            .stages
            .iter()
            .filter(|stage| stage.stage == name)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "expected one {name:?} stage in {document:?}"
        );
        found[0]
    }

    fn span_labels(stage: &StageReport) -> Vec<&str> {
        stage.spans.iter().map(|span| span.label.as_str()).collect()
    }

    // --- the gate: recording happens iff the switch asks for it -------------

    #[test]
    fn the_two_ways_of_writing_off_are_off_and_so_is_unset() {
        for raw in [None, Some(""), Some("   "), Some("0")] {
            assert_eq!(Mode::requested(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn one_asks_for_the_prose_and_json_asks_for_the_document() {
        assert_eq!(Mode::requested(Some("1")), Some(Mode::Prose));
        assert_eq!(Mode::requested(Some("json")), Some(Mode::Document));
        assert_eq!(Mode::requested(Some(" JSON ")), Some(Mode::Document));
    }

    #[test]
    fn any_other_on_value_keeps_the_prose_it_always_got() {
        // A habit of DEVLAUNCH_TIMING=true is not a request for the document.
        assert_eq!(Mode::requested(Some("true")), Some(Mode::Prose));
        assert_eq!(Mode::requested(Some("2")), Some(Mode::Prose));
    }

    // --- the prose summary --------------------------------------------------

    #[test]
    fn the_prose_summary_is_the_round_trips_then_the_total() {
        let mut registry = Registry::start(Mode::Prose, Seam::default(), now_epoch());
        registry.record("devpod status", Duration::from_millis(454));
        registry.record("devpod ssh", Duration::from_millis(1952));

        let lines = registry.finish().lines();

        assert_eq!(lines[0], "dl-timing: devpod status 0.454s");
        assert_eq!(lines[1], "dl-timing: devpod ssh 1.952s");
        assert_eq!(lines.len(), 3);
        assert!(
            lines[2].starts_with("dl-timing: total 0.0")
                && lines[2].ends_with(&format!("s ({TOTAL_EPOCH})")),
            "{:?}",
            lines[2]
        );
    }

    #[test]
    fn a_run_that_recorded_nothing_still_reports_its_total() {
        let lines = Registry::start(Mode::Prose, Seam::default(), now_epoch())
            .finish()
            .lines();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("dl-timing: total "), "{:?}", lines[0]);
    }

    #[test]
    fn the_prose_summary_carries_no_document_and_the_document_no_prose() {
        let prose = Registry::start(Mode::Prose, Seam::default(), now_epoch()).finish();
        assert!(prose.document().is_none());
        for line in prose.lines() {
            assert!(!line.contains(JSON_PREFIX), "{line:?}");
        }

        let document = recording().finish();
        assert_eq!(document.lines().len(), 1);
        assert!(document.lines()[0].starts_with(JSON_PREFIX));
        assert!(!document.lines()[0].contains("dl-timing: "));
    }

    #[test]
    fn prose_mode_keeps_its_flat_span_lines_through_a_stage() {
        // `=1` is the summary it always was: a stage around a span must not add
        // a line to it or rename one.
        let mut registry = Registry::start(Mode::Prose, Seam::default(), now_epoch());
        registry.enter(Stage::Tools);
        registry.record("devpod ssh", Duration::from_millis(500));
        registry.leave(Stage::Tools, Outcome::Ok);

        let lines = registry.finish().lines();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "dl-timing: devpod ssh 0.500s");
        assert!(lines[1].starts_with("dl-timing: total "));
    }

    // --- the document -------------------------------------------------------

    #[test]
    fn the_document_is_one_marked_line_of_parseable_json() {
        let line = recording().finish().lines().remove(0);

        let json = line
            .strip_prefix(&format!("{JSON_PREFIX} "))
            .expect("the marker, then the document");
        let parsed: serde_json::Value = serde_json::from_str(json).expect("one JSON object");
        assert!(parsed["total"].as_f64().expect("a total") >= 0.0);
        assert_eq!(parsed["stages"].as_array().expect("stages").len(), 0);
        assert!(!line.contains('\n'), "one line, whatever else is on stderr");
    }

    #[test]
    fn the_document_is_spelled_the_way_json_dumps_spells_it() {
        // The byte-comparability the `Document` docstring promises: Python writes
        // `{"total": 0.1, …}` and `serde_json::to_string` writes `{"total":0.1,…}`.
        // Measured against the Python build's own line, whose separators are
        // `json.dumps`' defaults.
        let mut registry = recording();
        registry.enter(Stage::Tools);
        registry.record("devpod ssh", Duration::from_millis(500));
        registry.leave(Stage::Tools, Outcome::Ok);
        let line = registry.finish().lines().remove(0);

        let json = line
            .strip_prefix(&format!("{JSON_PREFIX} "))
            .expect("the marker, then the document");
        assert!(
            json.starts_with(r#"{"total": "#),
            "a key and its value are separated by `: `, as Python's default is: {json}"
        );
        assert!(
            json.contains(r#", "total_epoch": "#),
            "and members by `, `: {json}"
        );
        assert!(
            json.contains(r#"{"label": "devpod ssh", "seconds": "#),
            "including inside the nested span reports: {json}"
        );
        assert!(
            json.ends_with(r#""spans": [{"label": "devpod ssh", "seconds": 0.5}]}]}"#),
            "right down to the innermost object: {json}"
        );
    }

    #[test]
    fn the_document_names_the_clock_its_total_came_from() {
        // The prose total carries that caveat inline; a consumer of the
        // document must not have to know it out of band.
        assert_eq!(document_of(recording()).total_epoch, TOTAL_EPOCH);
    }

    #[test]
    fn a_document_with_no_prewarm_omits_the_key_rather_than_emptying_it() {
        let line = recording().finish().lines().remove(0);
        assert!(!line.contains("prewarm"), "{line:?}");
    }

    #[test]
    fn the_vocabulary_is_the_ownership_boundary_stages() {
        assert_eq!(
            Stage::ALL.map(Stage::name),
            ["handoff", "host-prep", "devpod-up", "tools", "attach"]
        );
    }

    // --- stages -------------------------------------------------------------

    #[test]
    fn a_stage_carries_the_spans_recorded_inside_it() {
        let mut registry = recording();
        registry.enter(Stage::Tools);
        registry.record("devpod ssh", TICK);
        registry.leave(Stage::Tools, Outcome::Ok);

        let document = document_of(registry);

        assert_eq!(span_labels(stage_named(&document, "tools")), ["devpod ssh"]);
    }

    #[test]
    fn a_stage_totals_over_its_arm_not_just_over_its_spans() {
        // The stage is the arm's whole cost — the host-side work between the
        // round trips is time the owner spent too, and a trend that dropped it
        // would show parts that never add up to the total.
        let mut registry = recording();
        registry.enter(Stage::HostPrep);
        std::thread::sleep(TICK);
        registry.record("git ls-remote", TICK);
        std::thread::sleep(TICK);
        registry.leave(Stage::HostPrep, Outcome::Ok);

        let document = document_of(registry);

        let stage = stage_named(&document, "host-prep");
        let spans: f64 = stage.spans.iter().map(|span| span.seconds).sum();
        assert!(
            stage.seconds >= spans + TICK.as_secs_f64(),
            "{stage:?} against {spans}"
        );
    }

    #[test]
    fn a_stage_entered_again_totals_over_both_of_its_arms() {
        // One owner's work is not always one contiguous region — the token
        // fetch is host prep whenever it happens — so a stage accumulates
        // rather than reporting only its last visit.
        let mut registry = recording();
        for _ in 0..2 {
            registry.enter(Stage::HostPrep);
            std::thread::sleep(TICK);
            registry.leave(Stage::HostPrep, Outcome::Ok);
        }

        let document = document_of(registry);

        assert_eq!(stage_names(&document), ["host-prep"]);
        assert!(stage_named(&document, "host-prep").seconds >= 2.0 * TICK.as_secs_f64());
    }

    #[test]
    fn a_stage_inside_another_is_charged_to_the_inner_owner() {
        // Nesting must not double-count: `tools` runs inside the launch that
        // `devpod-up` brackets, and the seconds belong to one of them.
        let mut registry = recording();
        registry.enter(Stage::DevpodUp);
        registry.enter(Stage::Tools);
        std::thread::sleep(TICK);
        registry.leave(Stage::Tools, Outcome::Ok);
        registry.leave(Stage::DevpodUp, Outcome::Ok);

        let document = document_of(registry);

        assert!(stage_named(&document, "tools").seconds >= TICK.as_secs_f64());
        assert!(stage_named(&document, "devpod-up").seconds < TICK.as_secs_f64());
    }

    #[test]
    fn the_outer_stage_is_charged_again_once_the_inner_one_gives_it_back() {
        let mut registry = recording();
        registry.enter(Stage::DevpodUp);
        registry.enter(Stage::Tools);
        registry.leave(Stage::Tools, Outcome::Ok);
        std::thread::sleep(TICK);
        registry.leave(Stage::DevpodUp, Outcome::Ok);

        let document = document_of(registry);

        assert!(stage_named(&document, "devpod-up").seconds >= TICK.as_secs_f64());
    }

    #[test]
    fn a_span_lands_in_the_innermost_open_stage_only() {
        // Host prep is an owner, not a region of the timeline: the token trip
        // is the host's work wherever on the launch it falls, and the stage it
        // interrupts is not charged for it.
        let mut registry = recording();
        registry.enter(Stage::Attach);
        registry.enter(Stage::HostPrep);
        registry.record("gh auth token", TICK);
        registry.leave(Stage::HostPrep, Outcome::Ok);
        registry.leave(Stage::Attach, Outcome::Ok);

        let document = document_of(registry);

        assert_eq!(
            span_labels(stage_named(&document, "host-prep")),
            ["gh auth token"]
        );
        assert_eq!(
            span_labels(stage_named(&document, "attach")),
            [] as [&str; 0]
        );
    }

    #[test]
    fn a_stage_already_on_the_clock_is_not_re_entered() {
        // The arm is instrumented at several of its own entry points, and an
        // inner entry there is the same owner's same work.
        let mut registry = recording();
        assert_eq!(registry.enter(Stage::HostPrep), Entry::Opened);
        assert_eq!(registry.enter(Stage::HostPrep), Entry::AlreadyOpen);
        registry.record("git fetch", TICK);
        registry.leave(Stage::HostPrep, Outcome::Ok);

        let document = document_of(registry);

        assert_eq!(stage_names(&document), ["host-prep"]);
        assert_eq!(
            span_labels(stage_named(&document, "host-prep")),
            ["git fetch"],
            "the span is the one owner's, recorded once"
        );
    }

    #[test]
    fn a_stage_that_never_ran_is_absent_rather_than_zero() {
        // Absence is the "not reached" of the three-valued outcome: a stage
        // reporting 0.000s claims it ran and cost nothing, which is a different
        // and false statement.
        let mut registry = recording();
        registry.enter(Stage::Attach);
        registry.leave(Stage::Attach, Outcome::Ok);

        assert_eq!(stage_names(&document_of(registry)), ["attach"]);
    }

    #[test]
    fn a_stage_that_failed_reports_its_arm_up_to_the_failure() {
        let mut registry = recording();
        registry.enter(Stage::DevpodUp);
        std::thread::sleep(TICK);
        registry.leave(Stage::DevpodUp, Outcome::Failed);

        let document = document_of(registry);

        let stage = stage_named(&document, "devpod-up");
        assert_eq!(stage.outcome, "failed");
        assert!(stage.seconds >= TICK.as_secs_f64());
    }

    #[test]
    fn a_stage_that_returned_is_reported_ok() {
        let mut registry = recording();
        registry.enter(Stage::Attach);
        registry.leave(Stage::Attach, Outcome::Ok);

        assert_eq!(stage_named(&document_of(registry), "attach").outcome, "ok");
    }

    #[test]
    fn a_failed_arm_stays_failed_however_its_owner_is_entered_next() {
        // An owner's second arm succeeding does not unfail the first.
        let mut registry = recording();
        registry.enter(Stage::HostPrep);
        registry.leave(Stage::HostPrep, Outcome::Failed);
        registry.enter(Stage::HostPrep);
        registry.leave(Stage::HostPrep, Outcome::Ok);

        assert_eq!(
            stage_named(&document_of(registry), "host-prep").outcome,
            "failed"
        );
    }

    #[test]
    fn leaving_a_stage_that_was_never_entered_records_nothing() {
        let mut registry = recording();
        registry.leave(Stage::Tools, Outcome::Failed);

        assert_eq!(stage_names(&document_of(registry)), [] as [&str; 0]);
    }

    #[test]
    fn stages_closed_out_of_order_do_not_stay_on_the_clock() {
        // Guards are dropped innermost-first in structured code; a guard moved
        // somewhere that reverses that closes the stages above it too, rather
        // than leaving them charged to a clock nothing will stop.
        let mut registry = recording();
        registry.enter(Stage::DevpodUp);
        registry.enter(Stage::Tools);
        std::thread::sleep(TICK);
        registry.leave(Stage::DevpodUp, Outcome::Ok);
        std::thread::sleep(TICK);
        registry.leave(Stage::Tools, Outcome::Ok);

        let document = document_of(registry);

        assert!(stage_named(&document, "tools").seconds >= TICK.as_secs_f64());
        assert!(
            stage_named(&document, "tools").seconds < 2.0 * TICK.as_secs_f64(),
            "the inner stage stopped when the outer one closed it"
        );
    }

    #[test]
    fn the_stages_account_for_no_more_than_the_total() {
        // A decomposition that leaves the launch's time somewhere else is not a
        // decomposition — and the nesting must not charge one second twice.
        let mut registry = recording();
        registry.enter(Stage::DevpodUp);
        registry.enter(Stage::Tools);
        std::thread::sleep(TICK);
        registry.leave(Stage::Tools, Outcome::Ok);
        std::thread::sleep(TICK);
        registry.leave(Stage::DevpodUp, Outcome::Ok);

        let document = document_of(registry);

        let stages: f64 = document.stages.iter().map(|stage| stage.seconds).sum();
        assert!(stages <= document.total, "{stages} against {document:?}");
    }

    // --- the handoff stamp --------------------------------------------------

    #[test]
    fn a_stamp_reports_the_gap_between_it_and_dl_starting() {
        let now = now_epoch();
        let seam = Seam {
            keystroke: Some(now - 5.0),
            prewarm_fired: None,
        };

        let document = document_of(Registry::start(Mode::Document, seam, now));

        let handoff = stage_named(&document, "handoff");
        assert!((5.0..60.0).contains(&handoff.seconds), "{handoff:?}");
        assert_eq!(handoff.outcome, "ok");
        assert!(handoff.spans.is_empty());
    }

    #[test]
    fn the_handoff_is_reported_ahead_of_the_stages_dl_ran() {
        // It is the earliest thing the document describes, and a reader should
        // meet the stages in the order the launch met them.
        let now = now_epoch();
        let seam = Seam {
            keystroke: Some(now - 1.0),
            prewarm_fired: None,
        };
        let mut registry = Registry::start(Mode::Document, seam, now);
        registry.enter(Stage::HostPrep);
        registry.leave(Stage::HostPrep, Outcome::Ok);

        assert_eq!(
            stage_names(&document_of(registry)),
            ["handoff", "host-prep"]
        );
    }

    #[test]
    fn the_handoff_is_the_one_stage_that_lies_outside_the_total() {
        // It ends where `total` begins — it is the gap this process could not
        // have measured from inside itself — so a consumer adding the stages up
        // against the total is adding up the others.
        let now = now_epoch();
        let seam = Seam {
            keystroke: Some(now - 5.0),
            prewarm_fired: None,
        };

        let document = document_of(Registry::start(Mode::Document, seam, now));

        assert!(stage_named(&document, "handoff").seconds > document.total);
    }

    #[test]
    fn no_readable_stamp_reports_no_handoff_stage_at_all() {
        // Absent, not zero: reporting 0.000s would claim an instantaneous
        // handoff, and a trend cannot tell that apart from a real one.
        for raw in [
            None,
            Some(""),
            Some("   "),
            Some("a while ago"),
            Some("nan"),
            Some("inf"),
            Some("-inf"),
        ] {
            let seam = Seam::parse(raw, None);
            assert_eq!(seam.keystroke, None, "{raw:?}");
            let document = document_of(Registry::start(Mode::Document, seam, now_epoch()));
            assert_eq!(stage_names(&document), [] as [&str; 0], "{raw:?}");
        }
    }

    #[test]
    fn a_stamp_in_the_future_reports_no_handoff_stage() {
        // Two clocks that disagree produce a negative gap, which is not a
        // measurement of anything — so it is reported as the absence it is.
        let now = now_epoch();
        let seam = Seam {
            keystroke: Some(now + 60.0),
            prewarm_fired: None,
        };

        let document = document_of(Registry::start(Mode::Document, seam, now));

        assert_eq!(stage_names(&document), [] as [&str; 0]);
    }

    #[test]
    fn a_numeric_stamp_that_is_no_instant_reports_no_handoff_stage() {
        // A gap no `Duration` can hold is a number that parsed, not a reading.
        let seam = Seam::parse(Some("-1e300"), None);
        assert_eq!(seam.keystroke, Some(-1e300));

        let document = document_of(Registry::start(Mode::Document, seam, now_epoch()));

        assert_eq!(stage_names(&document), [] as [&str; 0]);
    }

    #[test]
    fn a_readable_stamp_is_read_as_the_instant_it_holds() {
        assert_eq!(
            Seam::parse(Some(" 1755000000.5 "), None).keystroke,
            Some(1755000000.5)
        );
        assert_eq!(Seam::parse(None, Some("1e3")).prewarm_fired, Some(1000.0));
    }

    #[test]
    fn the_prose_summary_is_untouched_by_a_stamp() {
        let now = now_epoch();
        let seam = Seam {
            keystroke: Some(now - 5.0),
            prewarm_fired: Some(now - 35.0),
        };

        let lines = Registry::start(Mode::Prose, seam, now).finish().lines();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("dl-timing: total "), "{:?}", lines[0]);
    }

    // --- the prewarm stamp --------------------------------------------------

    fn prewarmed(keystroke: Option<f64>, fired: Option<f64>, now: f64) -> Registry {
        Registry::start(
            Mode::Document,
            Seam {
                keystroke,
                prewarm_fired: fired,
            },
            now,
        )
    }

    #[test]
    fn no_prewarm_stamp_reports_no_prewarm_at_all() {
        let now = now_epoch();

        let document = document_of(prewarmed(Some(now - 5.0), None, now));

        assert_eq!(document.prewarm, None);
    }

    #[test]
    fn the_head_start_is_the_gap_the_prewarm_bought() {
        let now = now_epoch();

        let document = document_of(prewarmed(Some(now - 5.0), Some(now - 35.0), now));

        let head_start = document
            .prewarm
            .as_ref()
            .and_then(|prewarm| prewarm.head_start_seconds)
            .expect("a head start");
        assert!((29.0..31.0).contains(&head_start), "{head_start}");
    }

    #[test]
    fn without_the_keystroke_stamp_there_is_no_head_start_to_report() {
        // One stamp is not a gap: the head start is a difference, and half of a
        // difference is absent rather than zero.
        let now = now_epoch();

        let document = document_of(prewarmed(None, Some(now - 35.0), now));

        assert_eq!(document.prewarm, None, "and no prewarm key to hold it");
    }

    #[test]
    fn a_prewarm_fired_after_the_keystroke_reports_no_head_start() {
        // A prewarm that fired later than the keystroke gave no head start, and
        // a negative one is not a measurement to put in a trend.
        let now = now_epoch();

        let document = document_of(prewarmed(Some(now - 35.0), Some(now - 5.0), now));

        assert_eq!(document.prewarm, None);
    }

    #[test]
    fn a_launch_reports_the_shape_it_observed_against_the_prewarm_it_had() {
        let now = now_epoch();
        let mut registry = prewarmed(None, Some(now - 30.0), now);
        registry.observe_attach(AttachShape::Hit);

        let document = document_of(registry);

        let prewarm = document.prewarm.expect("a prewarm to report a shape of");
        assert_eq!(prewarm.shape, Some("hit"));
        assert_eq!(
            prewarm.head_start_seconds, None,
            "no keystroke stamp, so no gap to report"
        );
    }

    #[test]
    fn a_launch_with_nothing_prewarmed_claims_no_shape() {
        // Absent, not "miss": no prewarm was fired, so there is no prewarm to
        // report the outcome of.
        let now = now_epoch();
        let mut registry = prewarmed(Some(now - 5.0), None, now);
        registry.observe_attach(AttachShape::Miss);

        assert_eq!(document_of(registry).prewarm, None);
    }

    #[test]
    fn a_prewarm_with_both_facts_reports_both() {
        let now = now_epoch();
        let mut registry = prewarmed(Some(now - 5.0), Some(now - 35.0), now);
        registry.observe_attach(AttachShape::Partial);

        let prewarm = document_of(registry).prewarm.expect("a prewarm");

        assert_eq!(prewarm.shape, Some("partial"));
        assert!(prewarm.head_start_seconds.is_some());
    }

    #[test]
    fn every_shape_a_launch_can_observe_has_a_name() {
        assert_eq!(
            [AttachShape::Hit, AttachShape::Partial, AttachShape::Miss].map(AttachShape::name),
            ["hit", "partial", "miss"]
        );
    }

    #[test]
    fn the_prewarm_key_is_the_last_thing_in_the_document() {
        // Field order is Python's dict order, so the two binaries' documents
        // are byte-comparable.
        let now = now_epoch();
        let mut registry = prewarmed(Some(now - 5.0), Some(now - 35.0), now);
        registry.observe_attach(AttachShape::Hit);

        let line = registry.finish().lines().remove(0);

        let json = line
            .strip_prefix(&format!("{JSON_PREFIX} "))
            .expect("the marker, then the document");
        assert!(json.starts_with(r#"{"total": "#), "{json}");
        let keys: Vec<usize> = [r#""total_epoch": "#, r#""stages": "#, r#""prewarm": "#]
            .iter()
            .map(|key| json.find(key).unwrap_or_else(|| panic!("{key} in {json}")))
            .collect();
        assert!(keys[0] < keys[1] && keys[1] < keys[2], "{json}");
        assert!(
            json.ends_with(r#""head_start_seconds": 30.0, "shape": "hit"}}"#),
            "the prewarm's facts, last and in Python's order: {json}"
        );
    }

    #[test]
    fn seconds_are_reported_to_six_decimal_places() {
        // Python rounds every duration in the document with `round(x, 6)`.
        let mut registry = recording();
        registry.record("devpod up", Duration::from_nanos(1_234_567_891));

        let document = document_of(registry);

        assert_eq!(document.stages.len(), 0);
        assert_eq!(round6(Duration::from_nanos(1_234_567_891)), 1.234568);
    }

    // --- the process-global handle ------------------------------------------
    //
    // One registry per process, so these tests take [`exclusive`] rather than
    // running alongside each other *or* alongside the flow tests that record into
    // whatever is installed. Everything else above drives an owned registry and
    // needs none of it.

    fn global<T>(test: impl FnOnce() -> T) -> T {
        let held = exclusive();
        install(None);
        let outcome = test();
        install(None);
        drop(held);
        outcome
    }

    #[test]
    fn the_exclusion_is_reentrant_so_a_fixture_can_hold_it_without_deadlocking() {
        // The guard lives in test fixtures, and a test that builds two of them
        // asks twice on one thread. A plain mutex deadlocks against itself there,
        // which is why this is worth a test even though its failure is a hang.
        let outer = exclusive();
        let inner = exclusive();
        drop(inner);
        drop(outer);
        // And the lock really was released, so a third ask still succeeds.
        drop(exclusive());
    }

    #[test]
    fn the_veneer_records_spans_and_stages_against_the_installed_registry() {
        global(|| {
            install(Some(Registry::start(
                Mode::Document,
                Seam::default(),
                now_epoch(),
            )));

            {
                let _stage = stage(Stage::Tools);
                let _span = span("devpod ssh");
                std::thread::sleep(TICK);
            }
            let report = emit().expect("a report from an installed registry");

            let document = report.document().expect("the document mode was asked for");
            assert_eq!(stage_names(document), ["tools"]);
            assert_eq!(span_labels(stage_named(document, "tools")), ["devpod ssh"]);
            assert!(stage_named(document, "tools").seconds >= TICK.as_secs_f64());
        });
    }

    #[test]
    fn a_span_that_ended_in_a_panic_is_still_recorded() {
        global(|| {
            install(Some(Registry::start(
                Mode::Prose,
                Seam::default(),
                now_epoch(),
            )));

            let panicked = std::panic::catch_unwind(|| {
                let _span = span("devpod up");
                panic!("the spawn blew up");
            });
            assert!(panicked.is_err(), "the block really did panic");

            // A spawn that failed still took time, and dropping it would make
            // the parts add up to less than the total.
            let lines = emit().expect("a report").lines();
            assert!(lines[0].starts_with("dl-timing: devpod up "), "{lines:?}");
        });
    }

    #[test]
    fn a_stage_that_ended_in_a_panic_is_reported_failed() {
        global(|| {
            install(Some(Registry::start(
                Mode::Document,
                Seam::default(),
                now_epoch(),
            )));

            let panicked = std::panic::catch_unwind(|| {
                let _stage = stage(Stage::DevpodUp);
                panic!("up blew up");
            });
            assert!(panicked.is_err());

            let report = emit().expect("a report");
            let document = report.document().expect("a document");
            assert_eq!(stage_named(document, "devpod-up").outcome, "failed");
        });
    }

    #[test]
    fn an_arm_that_answered_err_is_reported_failed_without_panicking() {
        global(|| {
            install(Some(Registry::start(
                Mode::Document,
                Seam::default(),
                now_epoch(),
            )));

            let outcome: Result<(), &str> = stage_result(Stage::HostPrep, || Err("clone refused"));
            assert_eq!(outcome, Err("clone refused"));
            let ok: Result<&str, &str> = stage_result(Stage::Attach, || Ok("attached"));
            assert_eq!(ok, Ok("attached"));

            let report = emit().expect("a report");
            let document = report.document().expect("a document");
            assert_eq!(stage_named(document, "host-prep").outcome, "failed");
            assert_eq!(stage_named(document, "attach").outcome, "ok");
        });
    }

    #[test]
    fn a_wait_somebody_else_measured_is_recorded_under_its_label() {
        // The lock-wait seam: `locks` blocks inside a syscall and reports the
        // wait afterwards, so there is no scope left to wrap.
        global(|| {
            install(Some(Registry::start(
                Mode::Prose,
                Seam::default(),
                now_epoch(),
            )));

            let _stage = stage(Stage::HostPrep);
            record("lock wait", Duration::from_millis(250));
            drop(_stage);

            let lines = emit().expect("a report").lines();
            assert_eq!(lines[0], "dl-timing: lock wait 0.250s");
        });
    }

    #[test]
    fn with_nothing_installed_every_call_is_a_no_op() {
        // The off state stays free: new stages on the launch path must not make
        // an unmeasured run pay for them.
        global(|| {
            install(None);

            let ran = {
                let mut guard = stage(Stage::Tools);
                guard.fail();
                let _span = span("devpod up");
                record("lock wait", Duration::from_millis(250));
                observe_attach(AttachShape::Miss);
                true
            };

            assert!(ran, "the block still runs");
            assert!(emit().is_none(), "and there is nothing to report");
        });
    }

    #[test]
    fn emitting_twice_reports_once() {
        // `emit` stops recording, so a second command in the same process never
        // inherits the first one's spans.
        global(|| {
            install(Some(Registry::start(
                Mode::Prose,
                Seam::default(),
                now_epoch(),
            )));

            assert!(emit().is_some());
            let _span = span("devpod up");
            assert!(emit().is_none());
        });
    }

    #[test]
    fn installing_replaces_whatever_was_recording() {
        // One command's spans never leak into the next command's summary.
        global(|| {
            install(Some(Registry::start(
                Mode::Prose,
                Seam::default(),
                now_epoch(),
            )));
            record("devpod list", TICK);

            install(Some(Registry::start(
                Mode::Prose,
                Seam::default(),
                now_epoch(),
            )));

            let lines = emit().expect("a report").lines();
            assert_eq!(lines.len(), 1, "only the total: {lines:?}");
        });
    }

    #[test]
    fn begin_reads_the_environment_and_emit_answers_what_it_found() {
        // Read-only against the process environment, which every other test in
        // this binary shares: whichever way it is set, `begin` and `emit` agree
        // with each other about it. The rules themselves are pinned above, on
        // `Mode::requested` and `Seam::parse`.
        global(|| {
            begin();
            let asked = Mode::from_env();

            let report = emit();

            match asked {
                None => assert!(report.is_none(), "off, so nothing to report"),
                Some(mode) => {
                    let report = report.expect("the switch asked for a summary");
                    assert_eq!(report.document().is_some(), mode == Mode::Document);
                }
            }
        });
    }
}
