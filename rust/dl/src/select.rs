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
//! **One deliberate departure from Python's picker: it can take several rows.**
//! Python's `iterfzf(..., multi=False)` answered one workspace always. Here the
//! verb the selector was opened for decides ([`Arity`]): a verb that applies per
//! workspace and returns — `up`, `stop`, `rm`, `code`, `dotfiles` — lets TAB mark
//! any number of rows, so `dl rm` can clear five dead workspaces in one visit,
//! while the forms that end in a session still take exactly one.
//!
//! **And one departure from skim's defaults: the search bar is at the top.** skim
//! draws its query line at the bottom and grows the match list upward from it;
//! [`skim_options`] asks for `reverse`, so the prompt is the first line and the
//! matches read downward from it. Neither shape is Python's to keep — `iterfzf`
//! inherited whatever `fzf` was configured to do on the host — and this one puts
//! the first match next to the cursor instead of furthest from it.
//!
//! **The [`invitation`] is drawn inside the picker, as skim's header.** Python
//! printed it and then spawned fzf, and `dl` copied that — but skim's first act is
//! to switch to the alternate screen, which replaces the visible one, so a line
//! printed on the way in cannot be read while the picker it describes is up. For
//! [`Arity::Several`] that line is TAB's only documentation, so the multi-select
//! was there and undiscoverable. stdout keeps the sentence only for the run that
//! has no picker to put it on ([`Pick::NoTerminal`]).
//!
//! [`offered`] is that list and nothing else — a pure function of what devpod said,
//! which is what makes the spec testable without a terminal. [`pick`] is the
//! interactive half.

use std::borrow::Cow;
use std::sync::Arc;

use devlaunch_core::clients::devpod::Workspace;
use devlaunch_core::domain::workspace_state::NonEmpty;
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

/// How many rows one picker run may take.
///
/// Decided by the verb the selector was opened for
/// ([`Verb::several_at_once`](crate::cli::Verb::several_at_once)), not by the
/// picker: skim will happily multi-select for anything, and the limit is about
/// what the verb can do with the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arity {
    /// Enter takes the row under the cursor and nothing else.
    One,
    /// TAB marks any number of rows; Enter takes the marked set, or the row under
    /// the cursor when none are marked.
    Several,
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
    /// These workspaces, by id, in the order skim handed the rows back. One entry
    /// always under [`Arity::One`]; [`NonEmpty`] because an empty set of choices
    /// is [`Pick::Quit`], not a batch of nothing.
    Chose(NonEmpty<String>),
    /// The picker was opened and closed without a choice: Esc, Ctrl-C, or rows
    /// that named no workspace.
    Quit,
    /// devpod lists nothing, so there is nothing to offer.
    NoWorkspaces,
    /// There is no terminal to draw a picker on — `dl < /dev/null`, a cron job, a
    /// pipe. Python's fzf said `inappropriate ioctl for device` and answered
    /// nothing; this answers the same nothing without the subprocess.
    NoTerminal,
}

/// Offer these workspaces and wait for one — or, under [`Arity::Several`], any
/// number — to be chosen.
pub(crate) fn pick(workspaces: &[Workspace], arity: Arity) -> Pick {
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
    chosen(&offers, run_skim(&offers, arity))
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

/// The sentence that says what the rows are, and what the picker will take.
///
/// One function because the text has two destinations and they must not drift: it
/// is skim's header when there is a picker to draw it on, and stdout when there is
/// not. The wording is Python's (`dl.py::fuzzy_select_workspace`), plus the TAB
/// clause the multi-pick needed.
pub(crate) fn invitation(arity: Arity) -> &'static str {
    match arity {
        Arity::One => "Select workspace (type to filter):",
        Arity::Several => "Select workspaces (type to filter, TAB to mark several):",
    }
}

/// How the picker is drawn, and how many rows it will take.
///
/// A function rather than a literal inside [`run_skim`] because it is the one part
/// of the interactive half a test can read without a terminal: these options *are*
/// the picker's shape, so a lever that quietly does nothing is caught here rather
/// than by looking at it.
fn skim_options(arity: Arity) -> SkimOptions {
    SkimOptions {
        // skim's `--multi` when the verb takes several — TAB toggles a row, as it
        // does in fzf — and `iterfzf(options, multi=False)`'s one pick otherwise.
        // Input order preserved rather than re-sorted either way (Python's
        // `sort=False` -> `--no-sort`).
        multi: matches!(arity, Arity::Several),
        no_sort: true,
        // The search bar on the first line, with the matches reading downward from
        // it. skim's default draws the query at the *bottom* and grows the list
        // upward, which puts the first match furthest from where the eye already
        // is and moves the rows under it as the query narrows.
        //
        // Named as a layout and not as `reverse: true`, though the field beside it
        // is documented as shorthand for exactly this: the shorthand is expanded by
        // `SkimOptions::build`, and `dl` hands its options to `Skim::run_with`,
        // which never calls it. The flag on its own compiles, reads as the fix, and
        // changes nothing (skim 0.20.5).
        layout: String::from("reverse"),
        // The invitation, drawn *inside* the picker rather than printed before it.
        // skim's first act is `ESC [ ? 1049 h` — the alternate screen, which
        // replaces the visible screen wholesale — so a line printed on the way in
        // is gone for exactly as long as it is needed and comes back only once the
        // picker has exited. Under `reverse` the model splits the window
        // query_status / query / status / *header* / selection
        // (`src/model/mod.rs`), so this lands directly above the rows it describes.
        //
        // Unlike `reverse` next door, this field is read straight off the options
        // by `Header::with_options`, so it works without `SkimOptions::build` —
        // which is why the pty test is still the thing that proves it.
        header: Some(invitation(arity).to_owned()),
        ..Default::default()
    }
}

/// The rows skim was left on: empty when the picker was quit without an answer.
fn run_skim(offers: &[Offer], arity: Arity) -> Vec<String> {
    let options = skim_options(arity);
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for row in rows_of(offers) {
        // A send that fails means the reader is gone, which is a picker that is not
        // going to answer; the remaining rows are not worth a diagnostic.
        if tx.send(row).is_err() {
            break;
        }
    }
    // The reader stops at the end of the stream, and the stream ends when the last
    // sender is dropped.
    drop(tx);
    let Some(output) = Skim::run_with(&options, Some(rx)) else {
        return Vec::new();
    };
    if output.is_abort {
        return Vec::new();
    }
    output
        .selected_items
        .iter()
        .map(|item| item.output().into_owned())
        .collect()
}

/// The offers as skim items, each carrying its own position as the item index.
fn rows_of(offers: &[Offer]) -> Vec<Arc<dyn SkimItem>> {
    offers
        .iter()
        .enumerate()
        .map(|(index, offer)| {
            let row: Arc<dyn SkimItem> = Arc::new(Row {
                label: offer.label.clone(),
                index,
            });
            row
        })
        .collect()
}

/// Which workspaces the chosen rows name.
///
/// Python looks the label up in its `ws_map` and answers `None` when it is not
/// there; the same lookup is here, per row. A row naming no workspace is dropped
/// rather than read as a workspace called something else, and rows naming nothing
/// at all — none chosen, or none that map back — read as no choice.
fn chosen(offers: &[Offer], rows: Vec<String>) -> Pick {
    let picked = rows.iter().filter_map(|row| {
        offers
            .iter()
            .find(|offer| offer.label == *row)
            .map(|offer| offer.workspace_id.clone())
    });
    NonEmpty::of(picked).map_or(Pick::Quit, Pick::Chose)
}

/// One offered row, as skim reads it.
struct Row {
    label: String,
    /// The row's position among the offers. skim's multi-select keys every marked
    /// row by `(run, get_index())`, and `get_index()` defaults to 0 — so rows that
    /// do not carry their own index all collide on one key, and each TAB *removes*
    /// the previous mark instead of adding to it. One distinct index per row is
    /// what makes marking accumulate.
    index: usize,
}

impl SkimItem for Row {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.label)
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
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
            chosen(&offers, vec![offers[1].label.clone()]),
            Pick::Chose(one_id("from-an-image"))
        );
    }

    #[test]
    fn the_search_bar_is_at_the_top_with_the_list_reading_downward() {
        // skim's default layout draws the query line at the *bottom* of the picker
        // and grows the list upward from it, so the first match sits furthest from
        // the cursor and the rows move under the eye as the query is typed. `dl`
        // asks for `reverse`: prompt on the first line, matches reading downward
        // from it, first match nearest the prompt — the shape every other fuzzy
        // finder a user of this has met puts up.
        //
        // Both arities, because the layout is not the multi-select's business: a
        // single-pick `dl stop` is drawn exactly like a multi-pick `dl rm`.
        assert_eq!(skim_options(Arity::One).layout, "reverse");
        assert_eq!(skim_options(Arity::Several).layout, "reverse");
    }

    #[test]
    fn the_layout_is_asked_for_by_name_because_the_reverse_flag_is_inert() {
        // `SkimOptions` carries a `reverse: bool` next to `layout`, documented as
        // "shorthand for reverse layout" — and it is shorthand that only
        // `SkimOptions::build`/`SkimOptionsBuilder::build` ever expand. `dl` hands
        // its options straight to `Skim::run_with`, which never calls either, and
        // the model reads `options.layout` alone (skim 0.20.5,
        // `src/model/mod.rs`). So a `reverse: true` on its own would compile, read
        // as the fix, and draw the old picture.
        //
        // Pinned so that a later tidy-up cannot "simplify" the layout string into
        // the flag and silently put the search bar back at the bottom.
        let asked = skim_options(Arity::One);

        assert_eq!(asked.layout, "reverse");
        assert!(
            !asked.reverse,
            "the flag is not the lever; setting it instead of `layout` is the bug this pins"
        );
    }

    #[test]
    fn the_invitation_is_the_picker_s_own_header_and_names_tab_only_where_tab_works() {
        // The line that explains the rows travels with the picker rather than ahead
        // of it. Printing it first put it on the screen skim was about to replace
        // with the alternate one, so it was unreadable for the whole time the
        // picker was up — and for the multi-pick verbs that made TAB, the thing the
        // sentence exists to teach, discoverable nowhere at all.
        //
        // `tests/picker.rs` is what proves the header is *drawn*; this pins that
        // `dl` asks for it, and that the wording still follows the arity. Both
        // matter: an unspoken TAB clause on a single-pick picker is a lie, and a
        // missing one on a multi-pick picker is the defect.
        assert_eq!(
            skim_options(Arity::Several).header.as_deref(),
            Some("Select workspaces (type to filter, TAB to mark several):")
        );
        assert_eq!(
            skim_options(Arity::One).header.as_deref(),
            Some("Select workspace (type to filter):")
        );
        assert!(
            !invitation(Arity::One).contains("TAB"),
            "a picker that takes one row must not offer a key that does nothing on it"
        );
    }

    #[test]
    fn what_the_picker_will_take_is_the_arity_and_the_order_is_never_skim_s() {
        // The two options the layout change sits beside, so a mistyped struct
        // literal cannot trade one for another unnoticed.
        assert!(!skim_options(Arity::One).multi);
        assert!(skim_options(Arity::Several).multi);
        assert!(skim_options(Arity::One).no_sort);
        assert!(skim_options(Arity::Several).no_sort);
    }

    /// A `Pick::Chose` of exactly these ids, for the assertions below.
    fn ids(named: &[&str]) -> Pick {
        Pick::Chose(NonEmpty::of(named.iter().map(|id| (*id).to_owned())).expect("at least one id"))
    }

    fn one_id(named: &str) -> NonEmpty<String> {
        NonEmpty::of([named.to_owned()]).expect("one id")
    }

    #[test]
    fn every_row_carries_its_own_index_or_marking_cannot_accumulate() {
        // skim's multi-select keys each marked row by `(run, get_index())`, and the
        // trait's `get_index()` defaults to 0. Rows all answering 0 therefore share
        // one key, and every TAB after the first *removes* the previous mark
        // instead of adding to it — observed live: mark two workspaces, and only
        // the last one is acted on. Distinct indices are what make marking
        // accumulate, so they are the spec.
        let offers = offered(&listed(
            r#"[
                {"id": "first", "source": {"localFolder": "/a"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"},
                {"id": "second", "source": {"localFolder": "/b"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"},
                {"id": "third", "source": {"localFolder": "/c"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"}
            ]"#,
        ));

        assert_eq!(
            rows_of(&offers)
                .iter()
                .map(|row| row.get_index())
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn several_rows_map_to_several_workspaces_in_the_order_taken() {
        // The multi pick: every chosen row maps back, in the order the rows came
        // back, so `dl rm` applies to the workspaces in the order they were marked.
        let offers = offered(&listed(
            r#"[
                {"id": "first", "source": {"localFolder": "/a"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"},
                {"id": "second", "source": {"localFolder": "/b"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"},
                {"id": "third", "source": {"localFolder": "/c"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"}
            ]"#,
        ));

        assert_eq!(
            chosen(
                &offers,
                vec![offers[2].label.clone(), offers[0].label.clone()]
            ),
            ids(&["third", "first"])
        );
        // A row naming no workspace is dropped rather than sinking the rows that
        // do name one — the batch the user marked still happens.
        assert_eq!(
            chosen(
                &offers,
                vec!["something else".to_owned(), offers[1].label.clone()]
            ),
            ids(&["second"])
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
        // before anything is drawn, whichever arity asked.
        assert_eq!(pick(&none, Arity::One), Pick::NoWorkspaces);
        assert_eq!(pick(&none, Arity::Several), Pick::NoWorkspaces);
    }

    #[test]
    fn a_row_that_names_no_workspace_is_no_choice() {
        // Python's `ws_map.get(selected)`: a label the map has not got answers
        // `None`, and `None` is the help and exit 1.
        let offers = offered(&listed(&one("mine", r#"{"localFolder": "/p"}"#)));

        assert_eq!(chosen(&offers, Vec::new()), Pick::Quit);
        assert_eq!(
            chosen(&offers, vec!["something else".to_owned()]),
            Pick::Quit
        );
    }
}
