//! The embedded fuzzy picker: the workspace `dl` was not given one for.
//!
//! **Divergence row 6.** Python spawned `fzf` through `iterfzf`, which made a
//! launcher fail for a reason that had nothing to do with launching: no `fzf` on
//! PATH, or an `iterfzf` that could not be imported. Here the picker is
//! [`skim`](https://crates.io/crates/skim) linked into the binary, so there is
//! nothing to install and nothing to find.
//!
//! What it offers is Python's list, in Python's order and Python's spelling:
//! `dl.py::fuzzy_select_workspace` renders one row per workspace devpod lists as
//! `{id} | {kind} | {detail}`, where the last two come from `describe_source` — the
//! same reading `dl --ls` shows, so a workspace whose source devlaunch cannot read
//! is *offered* rather than quietly dropped from the list
//! (`test/unit/test_workspace_source.py::TestTheFuzzyPickerOffersEverySource`).
//!
//! [`offered`] is that list and nothing else — a pure function of what devpod said,
//! which is what makes the spec testable without a terminal. [`pick`] is the
//! interactive half.

use std::borrow::Cow;
use std::sync::Arc;

use devlaunch_core::clients::devpod::Workspace;
use devlaunch_core::flows::listing::describe_source;
use skim::prelude::*;

/// One row the picker offers, and the workspace it stands for.
///
/// Both halves together, because a label a user can pick that maps back to nothing
/// is a row that does nothing: Python keeps the same pair in its `ws_map`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Offer {
    pub(crate) label: String,
    pub(crate) workspace_id: String,
}

/// Every workspace devpod listed, in devpod's order, as the picker shows it.
///
/// No filtering of any kind: the picker is a view of `dl --ls`, so a workspace
/// devlaunch did not create and one whose source it cannot read are both offered.
/// The `|` separators and the spacing are the label's bytes, and they are Python's.
pub(crate) fn offered(workspaces: &[Workspace]) -> Vec<Offer> {
    workspaces
        .iter()
        .map(|workspace| {
            let source = describe_source(workspace.source());
            Offer {
                label: format!(
                    "{} | {} | {}",
                    workspace.id,
                    source.kind.word(),
                    source.detail
                ),
                workspace_id: workspace.id.clone(),
            }
        })
        .collect()
}

/// What the picker settled.
///
/// Four arms where Python has `Optional[str]`, because its `None` covers four
/// different situations and two of them have something to say: an empty list is
/// reported (`No workspaces found …`), and a run with no terminal cannot draw a
/// picker at all. All three of the non-answers end the same way — Python prints the
/// help and exits 1 — but which one happened is the caller's to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// This workspace, by id.
    Chose(String),
    /// The picker was opened and closed without a choice: Esc, Ctrl-C, or a row
    /// that named no workspace.
    Quit,
    /// devpod lists nothing, so there is nothing to offer.
    NoWorkspaces,
    /// There is no terminal to draw a picker on — `dl < /dev/null`, a cron job, a
    /// pipe. Python's fzf said `inappropriate ioctl for device` and answered
    /// nothing; this answers the same nothing without the subprocess.
    NoTerminal,
}

/// Offer these workspaces and wait for one to be chosen.
pub(crate) fn pick(workspaces: &[Workspace]) -> Pick {
    let offers = offered(workspaces);
    if offers.is_empty() {
        return Pick::NoWorkspaces;
    }
    // Asked before skim is entered, and not as politeness: skim opens `/dev/tty`
    // and unwraps the result, so a run without one would abort the process rather
    // than answer.
    if !a_terminal_exists() {
        return Pick::NoTerminal;
    }
    chosen(&offers, run_skim(&offers))
}

/// Whether this run has a terminal at all.
///
/// `/dev/tty` and not stdin, because that is the file skim opens: a `dl` whose
/// stdin is a pipe still has a terminal to draw on if it was started from one, and
/// that is a picker that works.
fn a_terminal_exists() -> bool {
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    {
        Ok(tty) => {
            use std::os::fd::AsRawFd as _;
            // SAFETY: `isatty` reads one descriptor this scope owns and returns a
            // flag; the file is closed at the end of it either way.
            unsafe { libc::isatty(tty.as_raw_fd()) == 1 }
        }
        Err(_) => false,
    }
}

/// The row skim was left on, if it was left on one.
fn run_skim(offers: &[Offer]) -> Option<String> {
    let options = SkimOptions {
        // `iterfzf(options, multi=False)`'s defaults: one pick, and the input order
        // preserved rather than re-sorted (Python's `sort=False` -> `--no-sort`).
        multi: false,
        no_sort: true,
        ..Default::default()
    };
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for offer in offers {
        let row: Arc<dyn SkimItem> = Arc::new(Row {
            label: offer.label.clone(),
        });
        // A send that fails means the reader is gone, which is a picker that is not
        // going to answer; the remaining rows are not worth a diagnostic.
        if tx.send(row).is_err() {
            break;
        }
    }
    // The reader stops at the end of the stream, and the stream ends when the last
    // sender is dropped.
    drop(tx);
    let output = Skim::run_with(&options, Some(rx))?;
    if output.is_abort {
        return None;
    }
    output
        .selected_items
        .first()
        .map(|item| item.output().into_owned())
}

/// Which workspace a chosen row names.
///
/// Python looks the label up in its `ws_map` and answers `None` when it is not
/// there; the same lookup is here, and a row naming no workspace reads as no
/// choice rather than as a workspace called something else.
fn chosen(offers: &[Offer], row: Option<String>) -> Pick {
    let Some(row) = row else {
        return Pick::Quit;
    };
    offers
        .iter()
        .find(|offer| offer.label == row)
        .map_or(Pick::Quit, |offer| Pick::Chose(offer.workspace_id.clone()))
}

/// One offered row, as skim reads it.
struct Row {
    label: String,
}

impl SkimItem for Row {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.label)
    }
}

#[cfg(test)]
mod tests {
    //! `test/unit/test_workspace_source.py::TestTheFuzzyPickerOffersEverySource`,
    //! which is the spec for what the picker offers: the labels, and that picking
    //! one maps back to a workspace. Both are the pure half of this module, and the
    //! workspaces are built the way a real run gets them — from a `devpod list`
    //! answer through the listing parser — so the label is pinned over the whole
    //! path a source travels rather than over a hand-built value.
    //!
    //! The interactive half needs a terminal, and is left to manual testing (M9).

    use super::*;

    use devlaunch_core::flows::listing::CommandContext;
    use devlaunch_test_support::{FakeRunner, Response};

    /// The workspaces devpod lists, from the JSON it would have printed.
    fn listed(listing: &str) -> Vec<Workspace> {
        let runner = FakeRunner::new().with_script(
            ["devpod", "list", "--output", "json"],
            Response::stdout(listing),
        );
        CommandContext::new(&runner)
            .workspaces()
            .expect("a listing")
    }

    /// One workspace, with the source object devpod recorded for it.
    fn one(id: &str, source: &str) -> String {
        format!(
            r#"[{{"id": "{id}", "source": {source}, "lastUsed": "2026-08-01T00:00:00Z",
                "provider": {{"name": "docker"}}, "ide": {{"name": "none"}},
                "context": "default"}}]"#
        )
    }

    #[test]
    fn a_workspace_devlaunch_cannot_read_is_still_offered_and_selectable() {
        // Python's own two rows, byte for byte: a local folder, and a source
        // devlaunch has no reading for — offered as the JSON devpod sent, spelled
        // the way Python's `json.dumps` spells it, separators included.
        let workspaces = listed(
            r#"[
                {"id": "mine", "source": {"localFolder": "/home/dev/myproject"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "from-an-image", "source": {"image": "ubuntu:24.04"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offers = offered(&workspaces);

        assert_eq!(
            offers.iter().map(|offer| &offer.label).collect::<Vec<_>>(),
            [
                "mine | local | /home/dev/myproject",
                "from-an-image | unknown | {\"image\": \"ubuntu:24.04\"}",
            ]
        );
        // Picking the row maps back to the workspace, which is what makes it an
        // offer rather than a line of text.
        assert_eq!(
            chosen(&offers, Some(offers[1].label.clone())),
            Pick::Chose("from-an-image".to_owned())
        );
    }

    #[test]
    fn a_git_source_is_offered_by_its_url() {
        let workspaces = listed(&one(
            "wf",
            r#"{"gitRepository": "https://github.com/blooop/devlaunch.git"}"#,
        ));

        assert_eq!(
            offered(&workspaces)[0].label,
            "wf | git | https://github.com/blooop/devlaunch.git"
        );
    }

    #[test]
    fn the_order_is_devpods_and_nothing_is_left_out() {
        // The picker is a view of the workspace list: no sorting, and no filtering
        // by whose workspace it is. `no_sort` is what keeps skim from re-ordering
        // what this hands it.
        let workspaces = listed(
            r#"[
                {"id": "zebra", "source": {"localFolder": "/z"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"},
                {"id": "alpha", "source": {"localFolder": "/a"}, "lastUsed": "x",
                 "provider": {"name": "podman"}, "ide": {"name": "none"},
                 "context": "other"}
            ]"#,
        );

        assert_eq!(
            offered(&workspaces)
                .iter()
                .map(|offer| offer.workspace_id.clone())
                .collect::<Vec<_>>(),
            ["zebra", "alpha"]
        );
    }

    #[test]
    fn an_empty_listing_offers_nothing_and_is_not_a_picker() {
        let none = listed("[]");

        assert!(offered(&none).is_empty());
        // And no terminal is opened to say so: nothing to pick from is answered
        // before anything is drawn.
        assert_eq!(pick(&none), Pick::NoWorkspaces);
    }

    #[test]
    fn a_row_that_names_no_workspace_is_no_choice() {
        // Python's `ws_map.get(selected)`: a label the map has not got answers
        // `None`, and `None` is the help and exit 1.
        let offers = offered(&listed(&one("mine", r#"{"localFolder": "/p"}"#)));

        assert_eq!(chosen(&offers, None), Pick::Quit);
        assert_eq!(
            chosen(&offers, Some("something else".to_owned())),
            Pick::Quit
        );
    }
}
