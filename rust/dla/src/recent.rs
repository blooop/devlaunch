//! The choices dla's pickers list first: the recent ones, per agent and setting.
//!
//! This is what keeps the pickers current without a release. The table in
//! `rewrite` suggests a few values for a first run, and from then on the rows a
//! person sees are the ones they chose, newest first, so the cursor starts on the
//! last choice and one Enter repeats it. A model that came out yesterday is typed
//! once and listed from then on.
//!
//! The file is `<devlaunch cache>/dla-recent.tsv`, one choice per line:
//! `<agent> TAB <setting> TAB <value>`, newest first, with an empty value for the
//! default. Plain text rather than JSON because dla's one dependency is `dl`, and
//! the format needs nothing a `split` cannot read.
//!
//! **Nothing in here can stop a launch.** A file that is missing, unreadable,
//! half-written or full of junk reads as no history, and a write that fails is
//! dropped. The worst a broken file costs is the suggestions in place of the recent
//! choices.

use std::path::{Path, PathBuf};

use crate::rewrite::Knob;

/// The file's name inside the devlaunch cache.
const FILE_NAME: &str = "dla-recent.tsv";

/// How many choices are kept per agent and setting.
const KEPT: usize = 8;

/// What a remembered choice is kept under.
#[derive(Clone, Copy)]
enum Key<'a> {
    /// The launcher picker's row: which agent, and for claude which login. It is
    /// no one agent's setting, because it is what chooses the agent.
    Launcher,
    Knob(&'a str, Knob),
}

impl<'a> Key<'a> {
    /// The agent and setting columns of the file.
    fn columns(self) -> (&'a str, &'static str) {
        match self {
            Key::Launcher => ("*", "agent"),
            Key::Knob(agent, Knob::Model) => (agent, "model"),
            Key::Knob(agent, Knob::Effort) => (agent, "effort"),
        }
    }
}

/// Where the file lives, or `None` with no cache directory to put it in.
pub(crate) fn path() -> Option<PathBuf> {
    dl::cache_dir().map(|dir| dir.join(FILE_NAME))
}

/// One remembered choice. `value` is `None` for the default, which passes no flag.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    agent: String,
    setting: String,
    value: Option<String>,
}

/// Every remembered choice, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Recent {
    entries: Vec<Entry>,
}

impl Recent {
    /// The file at `path`, or no history if it cannot be read.
    pub(crate) fn read(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let entries = text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let (agent, setting, value) = (fields.next()?, fields.next()?, fields.next()?);
                if fields.next().is_some() || agent.is_empty() || setting.is_empty() {
                    return None;
                }
                Some(Entry {
                    agent: agent.to_owned(),
                    setting: setting.to_owned(),
                    value: (!value.is_empty()).then(|| value.to_owned()),
                })
            })
            .collect();
        Recent { entries }
    }

    /// The launcher rows remembered, by [`Launcher::key`](crate::rewrite::Launcher::key),
    /// newest first.
    pub(crate) fn launchers(&self) -> Vec<String> {
        self.lookup(Key::Launcher).into_iter().flatten().collect()
    }

    /// Remember a launcher row as the newest.
    pub(crate) fn record_launcher(&mut self, key: &str) {
        self.remember(Key::Launcher, Some(key));
    }

    /// The choices remembered for this agent's knob, newest first.
    pub(crate) fn values(&self, agent: &str, knob: Knob) -> Vec<Option<String>> {
        self.lookup(Key::Knob(agent, knob))
    }

    /// Remember a choice for this agent's knob as the newest.
    pub(crate) fn record(&mut self, agent: &str, knob: Knob, value: Option<&str>) {
        self.remember(Key::Knob(agent, knob), value);
    }

    fn lookup(&self, key: Key<'_>) -> Vec<Option<String>> {
        let (agent, setting) = key.columns();
        self.entries
            .iter()
            .filter(|entry| entry.agent == agent && entry.setting == setting)
            .map(|entry| entry.value.clone())
            .collect()
    }

    /// Remember a choice as the newest, keeping at most [`KEPT`] under its key.
    ///
    /// A value holding a tab or a line break is not remembered, because the file
    /// could not read it back as one value.
    fn remember(&mut self, key: Key<'_>, value: Option<&str>) {
        if value.is_some_and(|value| value.contains(['\t', '\n', '\r'])) {
            return;
        }
        let (agent, setting) = key.columns();
        let entry = Entry {
            agent: agent.to_owned(),
            setting: setting.to_owned(),
            value: value.map(str::to_owned),
        };
        self.entries.retain(|kept| *kept != entry);
        self.entries.insert(0, entry);
        let mut seen = 0;
        self.entries.retain(|kept| {
            if kept.agent != agent || kept.setting != setting {
                return true;
            }
            seen += 1;
            seen <= KEPT
        });
    }

    /// Write the file, replacing it whole. Failures are dropped.
    ///
    /// Through a temporary file and a rename, so a launch that reads it while
    /// another writes it sees the old file or the new one, never half of one.
    pub(crate) fn write(&self, path: &Path) {
        let text: String = self
            .entries
            .iter()
            .map(|entry| {
                format!(
                    "{}\t{}\t{}\n",
                    entry.agent,
                    entry.setting,
                    entry.value.as_deref().unwrap_or("")
                )
            })
            .collect();
        let Some(directory) = path.parent() else {
            return;
        };
        let staged = directory.join(format!("{FILE_NAME}.{}", std::process::id()));
        let written = std::fs::create_dir_all(directory)
            .and_then(|()| std::fs::write(&staged, text))
            .and_then(|()| std::fs::rename(&staged, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&staged);
        }
    }
}

/// The rows a picker lists, in order: the recent choices, then the default if it
/// was not among them, then each suggestion not already listed.
///
/// The default is always a row, because it is the one choice that needs no name.
pub(crate) fn ordered(recent: Vec<Option<String>>, offered: &[&str]) -> Vec<Option<String>> {
    let mut rows = recent;
    if !rows.contains(&None) {
        rows.push(None);
    }
    for value in offered {
        let value = Some((*value).to_owned());
        if !rows.contains(&value) {
            rows.push(value);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(value: &str) -> Option<String> {
        Some(value.to_owned())
    }

    #[test]
    fn the_rows_are_the_recent_ones_then_the_default_then_the_suggestions() {
        assert_eq!(
            ordered(vec![some("max"), some("low")], &["low", "medium", "max"]),
            [some("max"), some("low"), None, some("medium")]
        );
    }

    #[test]
    fn a_default_chosen_last_time_is_listed_where_it_was_chosen() {
        assert_eq!(
            ordered(vec![None, some("opus")], &["sonnet"]),
            [None, some("opus"), some("sonnet")]
        );
    }

    #[test]
    fn a_first_run_lists_the_default_ahead_of_the_suggestions() {
        assert_eq!(ordered(Vec::new(), &["opus"]), [None, some("opus")]);
    }

    #[test]
    fn a_choice_is_moved_to_the_front_and_not_listed_twice() {
        let mut recent = Recent::default();
        recent.record("claude", Knob::Model, Some("opus"));
        recent.record("claude", Knob::Model, Some("sonnet"));
        recent.record("claude", Knob::Model, Some("opus"));
        assert_eq!(
            recent.values("claude", Knob::Model),
            [some("opus"), some("sonnet")]
        );
    }

    #[test]
    fn choices_are_kept_apart_per_agent_and_setting() {
        let mut recent = Recent::default();
        recent.record("claude", Knob::Model, Some("opus"));
        recent.record("codex", Knob::Model, Some("gpt-5.5"));
        recent.record("claude", Knob::Effort, Some("max"));
        assert_eq!(recent.values("claude", Knob::Model), [some("opus")]);
        assert_eq!(recent.values("codex", Knob::Model), [some("gpt-5.5")]);
        assert_eq!(recent.values("claude", Knob::Effort), [some("max")]);
    }

    #[test]
    fn only_the_newest_few_are_kept() {
        let mut recent = Recent::default();
        recent.record("codex", Knob::Effort, Some("low"));
        for index in 0..KEPT + 3 {
            recent.record("claude", Knob::Model, Some(&format!("model-{index}")));
        }
        let kept = recent.values("claude", Knob::Model);
        assert_eq!(kept.len(), KEPT);
        assert_eq!(kept[0], Some(format!("model-{}", KEPT + 2)));
        // Another agent's history is not what pays for this one's.
        assert_eq!(recent.values("codex", Knob::Effort), [some("low")]);
    }

    #[test]
    fn the_file_reads_back_what_was_written() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let file = scratch.path().join("deeper").join(FILE_NAME);
        let mut recent = Recent::default();
        recent.record("claude", Knob::Model, None);
        recent.record_launcher("claude/work");
        recent.write(&file);
        assert_eq!(Recent::read(&file), recent);
    }

    #[test]
    fn a_value_the_file_could_not_hold_is_not_remembered() {
        let mut recent = Recent::default();
        recent.record("claude", Knob::Model, Some("a\tb"));
        assert_eq!(recent, Recent::default());
    }

    #[test]
    fn a_missing_or_broken_file_is_no_history() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let file = scratch.path().join(FILE_NAME);
        assert_eq!(Recent::read(&file), Recent::default());
        std::fs::write(
            &file,
            "junk\n\tmodel\topus\nclaude\tmodel\topus\textra\nclaude\tmodel\tsonnet\n",
        )
        .expect("a file");
        assert_eq!(
            Recent::read(&file).values("claude", Knob::Model),
            [some("sonnet")]
        );
    }
}
