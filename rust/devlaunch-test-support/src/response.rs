//! What a fake spawn answered: one value that every mode of the runner can turn
//! into its own outcome.
//!
//! A [`Response`] is a scripted [`Outcome`] with the capture decision left out.
//! The same answer has to serve a call that reads the output, a call that lets
//! it go to the terminal, and a session that reads stderr a line at a time — so
//! it carries the text unconditionally and each mode takes what it is entitled
//! to. That is the one place a fake may hold text a real inherited-stdio run
//! never produces, and it is why the conversions below drop it rather than
//! handing an empty string to a caller that asked for none.

use devlaunch_runner::{CapturedText, DetachOutcome, Exit, OsFailure, Outcome};

/// A scripted answer to one spawn. The arms are [`Outcome`]'s arms, minus the
/// distinction between a captured and an inherited run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// The program ran and ended this way, having written this much.
    Ran {
        exit: Exit,
        stdout: String,
        stderr: String,
    },
    /// The program is not on PATH — `devpod` is not installed.
    ProgramNotFound,
    /// It was still running when its timeout ran out.
    TimedOut,
    /// The OS refused for some other reason.
    NotStarted(OsFailure),
}

impl Response {
    /// Exit 0, silently. What most calls do.
    pub fn ok() -> Self {
        Self::exited(0)
    }

    /// Exit 0, having written `text` to stdout — an answer a caller parses.
    pub fn stdout(text: impl Into<String>) -> Self {
        Self::Ran {
            exit: Exit::Code(0),
            stdout: text.into(),
            stderr: String::new(),
        }
    }

    /// Exit with `code`, silently.
    pub fn exited(code: i32) -> Self {
        Self::Ran {
            exit: Exit::Code(code),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Exit with `code`, having complained on stderr — the failure-injection
    /// channel the shim spells as a response-table entry.
    pub fn failed(code: i32, stderr: impl Into<String>) -> Self {
        Self::Ran {
            exit: Exit::Code(code),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    /// Died of `signal` rather than exiting.
    pub fn signalled(signal: i32) -> Self {
        Self::Ran {
            exit: Exit::Signal(signal),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Also write this to stderr.
    #[must_use]
    pub fn and_stderr(mut self, text: impl Into<String>) -> Self {
        if let Self::Ran { stderr, .. } = &mut self {
            *stderr = text.into();
        }
        self
    }

    /// What the child wrote to stderr, as the lines a session would be handed:
    /// no newlines, and no empty last line for a stream that ended with one.
    pub(crate) fn stderr_lines(&self) -> Vec<String> {
        match self {
            Self::Ran { stderr, .. } => stderr
                .split_inclusive('\n')
                .map(|line| line.trim_end_matches(['\n', '\r']).to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The outcome of a call that read both streams.
    pub(crate) fn captured(self) -> Outcome<CapturedText> {
        match self {
            Self::Ran {
                exit,
                stdout,
                stderr,
            } => Outcome::Ran {
                exit,
                io: CapturedText { stdout, stderr },
            },
            Self::ProgramNotFound => Outcome::ProgramNotFound,
            Self::TimedOut => Outcome::TimedOut,
            Self::NotStarted(failure) => Outcome::NotStarted(failure),
        }
    }

    /// The outcome of a call that captured nothing. The text is dropped here
    /// rather than travelling as a pair of empty strings.
    pub(crate) fn quiet(self) -> Outcome {
        match self {
            Self::Ran { exit, .. } => Outcome::Ran { exit, io: () },
            Self::ProgramNotFound => Outcome::ProgramNotFound,
            Self::TimedOut => Outcome::TimedOut,
            Self::NotStarted(failure) => Outcome::NotStarted(failure),
        }
    }

    /// The outcome of a detached spawn: it started, or it did not. How a
    /// scripted child would have *ended* is not a fact a detached spawn has.
    pub(crate) fn detached(self, pid: u32) -> DetachOutcome {
        match self {
            Self::Ran { .. } | Self::TimedOut => DetachOutcome::Started { pid },
            Self::ProgramNotFound => DetachOutcome::ProgramNotFound,
            Self::NotStarted(failure) => DetachOutcome::NotStarted(failure),
        }
    }
}
