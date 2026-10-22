//! The prompt editor, and the boot it overlaps.
//!
//! `dla <workspace>` with no prompt on a terminal does not go straight to the
//! agent any more: the workspace starts booting in the background *while* the
//! prompt is typed, so the minute a container takes to come up and the minute a
//! prompt takes to write are the same minute. The prompt is read from the
//! terminal rather than from argv, which is also the end of the shell-quoting
//! problem — nothing the user types here passes through a shell on the host.
//!
//! The boot is a separate **process**, not a thread, and that is the whole
//! design. It is the shape wf's prewarm already takes — a background
//! `dl <workspace> up` beside a foreground launch — so everything that makes two
//! launches of one workspace safe is already built and tested: the per-workspace
//! launch lock serializes them, and the foreground launch finds the container
//! running and fast-attaches. A thread could not do this: `devpod up` inherits
//! the process's stdout and stderr with no seam to intercept them, and the SIGINT
//! disposition `_exit`s the whole process.
//!
//! The child is this same binary re-entered through the internal `--boot-up`
//! argv (dla's one dependency is `dl`, and the one binary dla can find without
//! guessing at PATH is itself). Its output goes to a log file and is replayed to
//! stderr after the prompt is submitted, so the build's progress is seen — just
//! not interleaved with the typing. The prompt editor and the pickers hold the
//! terminal in raw mode, so a Ctrl-C there raises no signal: it is a byte dla
//! reads, and dla answers it with `BootChild::cancel`, which sends the child a
//! SIGINT of its own. The child's interrupt handler (the shared
//! `dl::install_signal_handlers` disposition) then kills its `devpod up` group
//! and unlinks its staged token file, so abandoning the editor tears the whole
//! boot down. The child is still left in dla's process group, so a Ctrl-C typed
//! outside raw mode, while the boot is waited on after the prompt is submitted,
//! reaches both processes as an ordinary terminal SIGINT.
//!
//! Every failure in here is a fallback, never an ending: a boot that could not
//! be spawned means the launch runs serially, exactly as it did before this
//! module existed. The feature is an overlap and an editor, not a new way for a
//! launch to fail.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::recent::{self, Recent};
use crate::rewrite::{self, DlaArgs, Environment, Knob, Launcher, Tuning};

/// The internal argv word the boot child is started with. Undocumented on
/// purpose: it is dla talking to itself, not a flag anyone types.
pub(crate) const BOOT_WORD: &str = "--boot-up";

/// How often the log is checked for new output while waiting for the boot.
const RELAY_PAUSE: Duration = Duration::from_millis(50);

/// The background boot: the child, its log, and how much of it has been relayed.
pub(crate) struct BootChild {
    child: Child,
    log: PathBuf,
    relayed: u64,
}

impl BootChild {
    /// Start `<this binary> --boot-up <boot args>` with its output parked in a
    /// log file.
    ///
    /// `None` on any failure — an unresolvable `current_exe`, an unwritable temp
    /// directory — and the caller launches serially, as dla always has. stdin is
    /// closed rather than inherited so the boot cannot eat a keystroke that
    /// belongs to the editor.
    pub(crate) fn spawn(boot_args: &[String]) -> Option<Self> {
        let me = std::env::current_exe().ok()?;
        let log =
            std::env::temp_dir().join(format!("devlaunch-dla-boot-{}.log", std::process::id()));
        let out = std::fs::File::create(&log).ok()?;
        let err = match out.try_clone() {
            Ok(err) => err,
            Err(_) => {
                let _ = std::fs::remove_file(&log);
                return None;
            }
        };
        let mut command = Command::new(me);
        command
            .arg(BOOT_WORD)
            .args(boot_args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        // The boot must not outlive this dla. A dla that is SIGKILLed never gets
        // to `cancel`, and its boot then goes on holding devpod's workspace lock
        // with nobody left to wait on it.
        dl::interrupted_with_this_process(&mut command);
        let spawned = command.spawn();
        let Ok(child) = spawned else {
            // The fallback path must not litter: the log was created for a boot
            // that never started.
            let _ = std::fs::remove_file(&log);
            return None;
        };
        Some(BootChild {
            child,
            log,
            relayed: 0,
        })
    }

    /// Stop the boot the way a terminal Ctrl-C would, and wait for it to end.
    ///
    /// SIGINT, so the boot's own handler kills its `devpod up` group and unlinks
    /// its staged token file. Its output is not relayed: it is the noise of a boot
    /// nobody wants any more.
    pub(crate) fn cancel(mut self) {
        dl::interrupt(self.child.id());
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log);
    }

    /// Wait for the boot to end, relaying its output to stderr as it lands, and
    /// say whether it succeeded.
    ///
    /// A boot that did not end cleanly is reported in one line and then *not*
    /// acted on: the foreground launch that follows either finds the workspace
    /// up after all, retries the `up` itself, or surfaces the same refusal with
    /// dl's own words — all better answers than this function guessing.
    pub(crate) fn finish(mut self) {
        let ended = loop {
            self.relay();
            match self.child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => std::thread::sleep(RELAY_PAUSE),
                Err(_) => break None,
            }
        };
        // The tail written between the last relay and the exit.
        self.relay();
        let _ = std::fs::remove_file(&self.log);
        match ended {
            Some(0) => {}
            Some(code) => {
                eprintln!("dla: the background boot exited {code}; launching in the foreground...");
            }
            None => {
                eprintln!("dla: the background boot was killed; launching in the foreground...");
            }
        }
    }

    /// Everything the log holds beyond what was already relayed, onto stderr.
    ///
    /// Bytes, not lines: devpod's progress output carries carriage returns and
    /// partial lines, and reproducing them as they were written is what makes
    /// the replayed build look like the build.
    fn relay(&mut self) {
        use std::io::{Read as _, Seek as _, SeekFrom};

        let Ok(mut file) = std::fs::File::open(&self.log) else {
            return;
        };
        if file.seek(SeekFrom::Start(self.relayed)).is_err() {
            return;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_ok() && !bytes.is_empty() {
            self.relayed += bytes.len() as u64;
            let mut err = std::io::stderr();
            let _ = err.write_all(&bytes);
            let _ = err.flush();
        }
    }
}

/// Whether the agent, model and effort pickers run before the editor.
///
/// Only a bare `dla`, or a line of flags with no workspace, asks. A line that
/// names its workspace already said what it wanted, so it gets the editor alone
/// and every setting it left open stays the agent's default.
#[derive(Clone, Copy)]
pub(crate) enum Pickers {
    Ask,
    Skip,
}

/// How the interactive flow ended.
pub(crate) enum Collected {
    /// Launch this line, once the boot beside it, if any, has finished.
    Launch(Box<DlaArgs>, Option<BootChild>),
    /// A picker was cancelled, or Ctrl-C was typed at the prompt editor. The
    /// boot was stopped and nothing launches.
    Cancelled,
}

/// The interactive default, as one decision: boot in the background and collect
/// the prompt from the terminal, or hand the line back untouched.
///
/// The flow triggers only when every one of these holds — an agent line (a verb
/// line starts no agent and has nothing to ask for), an empty prompt (an inline
/// prompt was the answer already), a terminal on stdin and stdout (a pipe has
/// nobody typing; `DEVLAUNCH_NO_TTY=1` is the explicit opt-out), and a boot
/// child that actually spawned (anything less falls back to the serial launch
/// dla has always been).
///
/// An empty submission — a bare Enter, or Ctrl-D — leaves the prompt empty,
/// which is the agent's plain session: the old bare-`dla` behaviour is one
/// keystroke away, not gone.
///
/// `argv` and `environment` are what `parsed` was parsed from: the agent picker
/// builds its rows by parsing the line again with each row's flags in front.
/// `pickers` says whether those pickers run at all.
pub(crate) fn collect_prompt(
    parsed: DlaArgs,
    argv: &[String],
    environment: Environment<'_>,
    pickers: Pickers,
) -> Collected {
    let promptless_agent = matches!(
        &parsed.task,
        crate::rewrite::Task::Agent { prompt, .. } if prompt.is_empty()
    );
    if !promptless_agent || !dl::interactive_terminal() {
        return Collected::Launch(Box::new(parsed), None);
    }
    let Some(boot) = BootChild::spawn(&crate::rewrite::build_boot_args(&parsed)) else {
        return Collected::Launch(Box::new(parsed), None);
    };
    // Name the pane and the tab now, because the launch that would name them is
    // behind the editor and the editor is where the waiting happens. Deliberately
    // *after* the boot spawns and before the banner: the boot is what the name is
    // about, and a name written in front of a spawn that failed would be a name for
    // a launch that then runs serially and names itself anyway.
    //
    // The boot child cannot do this for us. Its stdout and stderr are a log file,
    // so `naming_gate` refuses it a name, and rightly: an OSC escape written into a
    // log is not a title. That gate is also why this is a foreground call rather
    // than something handed to `BootChild`.
    dl::name_before_launch(&parsed.spec);
    // The pickers come after the boot has started, so the minute they take is
    // also spent booting. Nothing they choose reaches the boot: `up` takes no
    // model, and `--claude-profile` is read when the session starts, not at `up`.
    let parsed = match pickers {
        Pickers::Skip => parsed,
        Pickers::Ask => {
            let Some(parsed) = settle(parsed, argv, environment) else {
                boot.cancel();
                return Collected::Cancelled;
            };
            parsed
        }
    };
    banner(&parsed);
    match dl::read_prompt() {
        dl::Submission::Text(typed) => {
            Collected::Launch(Box::new(parsed.with_prompt(typed)), Some(boot))
        }
        // The editor holds the terminal in raw mode, so Ctrl-C is a key there as
        // it is in a picker, and ends the run the same way.
        dl::Submission::Cancelled => {
            boot.cancel();
            Collected::Cancelled
        }
    }
}

/// What one picker settled.
enum Asked {
    /// This value, or `None` for the default, which passes no flag.
    Chose(Option<String>),
    /// Nothing to ask about, or no terminal to ask on. The line is left as it was.
    Skipped,
    /// Esc or Ctrl-C: the launch is off.
    Cancelled,
}

/// Ask for each setting the line left open, in one fixed order: the agent (and
/// for claude its login), the model, the effort. `None` is a cancel.
///
/// A setting a flag already gave is not asked for, and neither is one the agent
/// does not have. Every choice is remembered, so the next launch lists it first.
fn settle(parsed: DlaArgs, argv: &[String], environment: Environment<'_>) -> Option<DlaArgs> {
    let file = recent::path();
    let mut recent = file.as_deref().map(Recent::read).unwrap_or_default();
    let parsed = match ask_agent(&parsed, argv, environment, &mut recent) {
        Picked::Cancelled => return None,
        Picked::Kept => parsed,
        // The spec `parsed` holds may be a pull request link already resolved to a
        // branch, which a fresh parse of argv would undo.
        Picked::Line(line) => line.with_spec(parsed.spec.clone()),
    };
    let (Some(agent), Some(tuning)) = (parsed.agent(), parsed.tuning()) else {
        return Some(parsed);
    };
    let agent = agent.to_owned();
    let mut tuning = tuning.clone();
    for knob in Knob::ALL {
        if !rewrite::takes(&agent, knob) {
            continue;
        }
        let slot = match knob {
            Knob::Model => &mut tuning.model,
            Knob::Effort => &mut tuning.effort,
        };
        if slot.is_none() {
            match ask_value(&agent, knob, &recent) {
                Asked::Cancelled => return None,
                Asked::Skipped => continue,
                Asked::Chose(value) => *slot = value,
            }
        }
        recent.record(&agent, knob, slot.as_deref());
    }
    if let Some(file) = &file {
        recent.write(file);
    }
    Some(parsed.with_tuning(tuning))
}

/// What the agent picker settled.
enum Picked {
    /// The line as it was: one row or none to choose from, or no terminal.
    Kept,
    /// The line again, with the chosen row's flags in front.
    Line(Box<DlaArgs>),
    Cancelled,
}

/// The agent picker: one row per Claude login, then one for each other agent.
///
/// Not asked when there is at most one row, such as a line that typed `--codex`.
/// The first run lists the default agent's rows first; after that the rows chosen
/// most recently lead. Typed text that is no row is not an agent, so the picker
/// asks again.
fn ask_agent(
    parsed: &DlaArgs,
    argv: &[String],
    environment: Environment<'_>,
    recent: &mut Recent,
) -> Picked {
    let (heading, offers) = dl::claude_profile_offers();
    let named: Vec<String> = offers
        .iter()
        .map(|offer| offer.name.clone())
        .filter(|name| name != dl::DEFAULT_CLAUDE_PROFILE)
        .collect();
    let mut rows = rewrite::launchers(argv, environment, parsed, &named);
    if rows.len() < 2 {
        return Picked::Kept;
    }
    // A stable sort, so rows with the same rank keep the table's order.
    let remembered = recent.launchers();
    let rank = |row: &Launcher| {
        remembered
            .iter()
            .position(|key| *key == row.key())
            .unwrap_or(remembered.len() + usize::from(parsed.agent() != Some(row.agent)))
    };
    rows.sort_by_key(|row| rank(row));

    let width = rows.iter().map(|row| row.agent.len()).max().unwrap_or(0);
    let login = |row: &Launcher| {
        let name = row.profile.as_deref().unwrap_or(dl::DEFAULT_CLAUDE_PROFILE);
        offers
            .iter()
            .find(|offer| rewrite::takes_claude_login(row.agent) && offer.name == name)
            .map(|offer| offer.label.clone())
    };
    let labels: Vec<String> = rows
        .iter()
        .map(|row| match login(row) {
            Some(label) => format!("{:<width$}  {label}", row.agent),
            None => row.agent.to_owned(),
        })
        .collect();
    let columns = if rows.iter().any(|row| login(row).is_some()) {
        format!("{:<width$}  {heading}", "AGENT")
    } else {
        "AGENT".to_owned()
    };
    let header =
        format!("Agent for this launch. Type to filter. Esc cancels the launch.\n{columns}");
    loop {
        match dl::choose(&header, &labels) {
            dl::Choice::Row(index) => {
                let row = &rows[index];
                let Ok(line) = rewrite::relaunched(argv, environment, row) else {
                    return Picked::Kept;
                };
                recent.record_launcher(&row.key());
                return Picked::Line(Box::new(line));
            }
            dl::Choice::Typed(_) => {}
            dl::Choice::Cancelled => return Picked::Cancelled,
            dl::Choice::NoTerminal => return Picked::Kept,
        }
    }
}

/// One picker over `rows`, drawn with `label`. Asked again after typed text that
/// cannot be a value, such as a flag.
fn ask(header: &str, rows: &[Option<String>], label: impl Fn(&Option<String>) -> String) -> Asked {
    let labels: Vec<String> = rows.iter().map(label).collect();
    loop {
        match dl::choose(header, &labels) {
            dl::Choice::Row(index) => return Asked::Chose(rows[index].clone()),
            dl::Choice::Typed(typed) if rewrite::usable_value(&typed) => {
                return Asked::Chose(Some(typed));
            }
            dl::Choice::Typed(_) => {}
            dl::Choice::Cancelled => return Asked::Cancelled,
            dl::Choice::NoTerminal => return Asked::Skipped,
        }
    }
}

/// The row that stands for "pass no flag".
const DEFAULT_ROW: &str = "default (the agent's own)";

/// The model or effort picker.
fn ask_value(agent: &str, knob: Knob, recent: &Recent) -> Asked {
    let rows = recent::ordered(
        recent.values(agent, knob),
        rewrite::suggestions(agent, knob),
    );
    let header = format!(
        "{} for {agent}. Type to filter, or type a name that is not listed.\n\
         Alt-Enter uses the text as typed. Esc cancels the launch.",
        match knob {
            Knob::Model => "Model",
            Knob::Effort => "Effort",
        }
    );
    ask(&header, &rows, |row| {
        row.clone().unwrap_or_else(|| DEFAULT_ROW.to_owned())
    })
}

/// What the line settled beyond the agent, for the banner: ` (account work, model
/// opus)`, or nothing when every setting is the default.
fn chosen(parsed: &DlaArgs) -> String {
    let default = Tuning::default();
    let tuning = parsed.tuning().unwrap_or(&default);
    let parts: Vec<String> = [
        ("account", parsed.claude_profile()),
        ("model", tuning.model.as_deref()),
        ("effort", tuning.effort.as_deref()),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| format!("{name} {value}")))
    .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// The one line the editor shows before the read, naming what is booting, which
/// agent gets the prompt, and both ways out.
pub(crate) fn banner(parsed: &DlaArgs) {
    let agent = parsed.agent().unwrap_or_default();
    eprintln!(
        "Booting {} in the background. Type the prompt for {agent}{} and press Enter to \
         launch; an empty Enter starts a plain session. Alt-Enter or Ctrl-J adds a line, \
         and a paste keeps its line breaks.",
        parsed.spec,
        chosen(parsed)
    );
}
