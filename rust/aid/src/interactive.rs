//! The prompt editor, and the boot it overlaps.
//!
//! `aid <workspace>` with no prompt on a terminal does not go straight to the
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
//! argv (aid's one dependency is `dl`, and the one binary aid can find without
//! guessing at PATH is itself). Its output goes to a log file and is replayed to
//! stderr after the prompt is submitted, so the build's progress is seen — just
//! not interleaved with the typing. The child is deliberately left in aid's
//! process group: a terminal Ctrl-C mid-editing reaches both processes, and the
//! child's own interrupt handler (the shared `dl::install_signal_handlers`
//! disposition) kills its `devpod up` group and unlinks its staged token file,
//! so abandoning the editor tears the whole boot down with no new machinery.
//!
//! Every failure in here is a fallback, never an ending: a boot that could not
//! be spawned means the launch runs serially, exactly as it did before this
//! module existed. The feature is an overlap and an editor, not a new way for a
//! launch to fail.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::rewrite::AidArgs;

/// The internal argv word the boot child is started with. Undocumented on
/// purpose: it is aid talking to itself, not a flag anyone types.
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
    /// directory — and the caller launches serially, as aid always has. stdin is
    /// closed rather than inherited so the boot cannot eat a keystroke that
    /// belongs to the editor.
    pub(crate) fn spawn(boot_args: &[String]) -> Option<Self> {
        let me = std::env::current_exe().ok()?;
        let log =
            std::env::temp_dir().join(format!("devlaunch-aid-boot-{}.log", std::process::id()));
        let out = std::fs::File::create(&log).ok()?;
        let err = match out.try_clone() {
            Ok(err) => err,
            Err(_) => {
                let _ = std::fs::remove_file(&log);
                return None;
            }
        };
        let spawned = Command::new(me)
            .arg(BOOT_WORD)
            .args(boot_args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn();
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
                eprintln!("aid: the background boot exited {code}; launching in the foreground...");
            }
            None => {
                eprintln!("aid: the background boot was killed; launching in the foreground...");
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

/// The interactive default, as one decision: boot in the background and collect
/// the prompt from the terminal, or hand the line back untouched.
///
/// The flow triggers only when every one of these holds — an agent line (a verb
/// line starts no agent and has nothing to ask for), an empty prompt (an inline
/// prompt was the answer already), a terminal on stdin and stdout (a pipe has
/// nobody typing; `DEVLAUNCH_NO_TTY=1` is the explicit opt-out), and a boot
/// child that actually spawned (anything less falls back to the serial launch
/// aid has always been).
///
/// An empty submission — a bare Enter, or Ctrl-D — leaves the prompt empty,
/// which is the agent's plain session: the old bare-`aid` behaviour is one
/// keystroke away, not gone.
pub(crate) fn collect_prompt(parsed: AidArgs) -> (AidArgs, Option<BootChild>) {
    let promptless_agent = matches!(
        &parsed.task,
        crate::rewrite::Task::Agent { prompt, .. } if prompt.is_empty()
    );
    if !promptless_agent || !dl::interactive_terminal() {
        return (parsed, None);
    }
    let Some(boot) = BootChild::spawn(&crate::rewrite::build_boot_args(&parsed)) else {
        return (parsed, None);
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
    banner(&parsed);
    let typed = dl::read_terminal_submission();
    (parsed.with_prompt(typed), Some(boot))
}

/// The one line the editor shows before the read, naming what is booting, which
/// agent gets the prompt, and both ways out.
pub(crate) fn banner(parsed: &AidArgs) {
    let agent = parsed.agent().unwrap_or_default();
    eprintln!(
        "Booting {} in the background. Type the prompt for {agent} and press Enter to \
         launch; an empty Enter starts a plain session.",
        parsed.spec
    );
    eprint!("> ");
    let _ = std::io::stderr().flush();
}
