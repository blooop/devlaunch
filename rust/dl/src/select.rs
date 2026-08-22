//! The embedded fuzzy picker: the workspace `dl` was not given one for.
//!
//! **Divergence row 6.** Python spawned `fzf` through `iterfzf`, which made a
//! launcher fail for a reason that had nothing to do with launching: no `fzf` on
//! PATH, or an `iterfzf` that could not be imported. Here the picker is
//! [`skim`](https://crates.io/crates/skim) linked into the binary, so there is
//! nothing to install and nothing to find.
//!
//! What it offers is Python's list and Python's order, with one row per workspace
//! devpod lists — no filtering of any kind, so a workspace whose source devlaunch
//! cannot read is *offered* rather than quietly dropped
//! (`test/unit/test_workspace_source.py::TestTheFuzzyPickerOffersEverySource`).
//!
//! **The columns are not Python's.** `dl.py::fuzzy_select_workspace` drew
//! `{id} | {kind} | {detail}`, where the last two came from `describe_source`; this
//! draws `{owner} | {id}`. Both of the columns that went were answering questions
//! nobody standing at this picker is asking: `kind` reads `local` for every
//! workspace dl makes, since dl always hands devpod a path, and `detail` is the
//! clone directory dl chose and manages — a long, mechanically derived path whose
//! own last component is already the id in the column beside it. What is *missing*
//! from an id is the owner: an id is `<repo-slug>-<ref-slug>-<suffix>`
//! ([`devlaunch_core::domain::workspace_id`]) and carries no owner at all, so a
//! fork and its upstream are two rows spelled the same. [`owner_of`] derives it
//! from the source devpod already reported — no records opened for it — and the
//! column is padded to a common width so the ids line up under each other.
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
//! [`offered`] is that list and nothing else — a pure function of what devpod said
//! and where dl's cache is, which is what makes the spec testable without a
//! terminal. [`pick`] is the interactive half.

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use devlaunch_core::clients::devpod::Workspace;
use devlaunch_core::domain::workspace_state::NonEmpty;
use devlaunch_core::flows::listing::owner_of;
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

/// What the owner column says for a workspace whose source names no owner: one
/// opened from a path or a URL that is not GitHub's.
///
/// A dash rather than an empty cell, because the column is padded — a blank there
/// reads as a column that failed to draw, where a dash reads as an answer.
const NO_OWNER: &str = "-";

/// Every workspace devpod listed, in devpod's order, as the picker shows it.
///
/// No filtering of any kind: the picker is a view of `dl --ls`, so a workspace
/// devlaunch did not create and one whose source it cannot read are both offered.
///
/// The owner column is padded to the widest owner *in this list*, which is why the
/// labels are built here in one pass over all of them rather than one workspace at
/// a time: alignment is a fact about the set of rows, not about any one of them.
///
/// `cache_dir` is where dl keeps its clones, and it is what [`owner_of`] reads a
/// clone's owner out of the layout with — the same directory `--purge` decides
/// ownership by, so the two cannot disagree about which workspaces are dl's.
pub(crate) fn offered(workspaces: &[Workspace], cache_dir: &Path) -> Vec<Offer> {
    let owners: Vec<String> = workspaces
        .iter()
        .map(|workspace| owner_of(workspace, cache_dir).unwrap_or_else(|| NO_OWNER.to_owned()))
        .collect();
    // Characters and not bytes: a non-ASCII owner is one column per character on
    // the terminal, and `{:width$}` counts the same way.
    let width = owners
        .iter()
        .map(|owner| owner.chars().count())
        .max()
        .unwrap_or(0);
    workspaces
        .iter()
        .zip(owners)
        .map(|(workspace, owner)| Offer {
            label: format!("{owner:width$} | {}", workspace.id),
            workspace_id: workspace.id.clone(),
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
pub(crate) fn pick(workspaces: &[Workspace], arity: Arity, cache_dir: &Path) -> Pick {
    let offers = offered(workspaces, cache_dir);
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

    /// Where dl keeps its clones in these tests: the owner column is read out of
    /// the layout under it, and out of nothing under any other directory.
    const CACHE: &str = "/home/dev/.cache/devlaunch";

    /// That cache as a path, which is what every call here passes.
    fn cache() -> &'static Path {
        Path::new(CACHE)
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
        // Python's own two rows: somebody's project directory, and a source
        // devlaunch has no reading for. Neither names an owner — one is a path dl
        // did not clone, the other is an image reference — so both take the dash,
        // and both are still *offered*, which is the point of the test.
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

        let offers = offered(&workspaces, cache());

        assert_eq!(
            offers.iter().map(|offer| &offer.label).collect::<Vec<_>>(),
            ["- | mine", "- | from-an-image"]
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
        let offers = offered(
            &listed(
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
            ),
            cache(),
        );

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
        let offers = offered(
            &listed(
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
            ),
            cache(),
        );

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
    fn a_git_source_is_offered_under_the_owner_its_url_names() {
        let workspaces = listed(&one(
            "wf",
            r#"{"gitRepository": "https://github.com/blooop/devlaunch.git"}"#,
        ));

        assert_eq!(offered(&workspaces, cache())[0].label, "blooop | wf");
    }

    #[test]
    fn a_clone_of_dls_own_is_offered_under_the_owner_its_directory_names() {
        // The row a user of `dl` actually sees: every workspace `dl owner/repo`
        // makes is a clone at `<cache>/repos/<owner>/<repo>/<workspace id>` handed
        // to devpod as a path, so the owner is read back out of the layout — no
        // records opened, no config read, no disk touched.
        let workspaces = listed(&one(
            "devlaunch-main-zovomobo",
            r#"{"localFolder": "/home/dev/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache())[0].label,
            "blooop | devlaunch-main-zovomobo"
        );
    }

    #[test]
    fn a_directory_that_is_not_a_clone_of_dls_names_no_owner() {
        // `dl ~/dev/myproject`, and it is the case that catches a rule read off the
        // shape alone: the path is three components deep like any clone *and* its
        // leaf is the workspace's id, because devpod names a path workspace after
        // its directory. Read as dl's layout it would say the owner is `dev`, which
        // is a fabrication — nobody's repository is owned by a path component of
        // somebody's home directory. Being outside dl's cache is what settles it.
        let workspaces = listed(&one(
            "myproject",
            r#"{"localFolder": "/home/dev/myproject"}"#,
        ));

        assert_eq!(offered(&workspaces, cache())[0].label, "- | myproject");
    }

    #[test]
    fn a_directory_under_the_cache_that_is_not_shaped_like_a_clone_names_no_owner() {
        // The other half of the rule. Inside the cache, but its leaf is not this
        // workspace's id — so it is not the `<owner>/<repo>/<workspace id>` layout
        // dl writes, and there is no owner to be read out of it.
        let workspaces = listed(&one(
            "mine",
            r#"{"localFolder": "/home/dev/.cache/devlaunch/repos/blooop/devlaunch/somewhere-else"}"#,
        ));

        assert_eq!(offered(&workspaces, cache())[0].label, "- | mine");
    }

    #[test]
    fn a_directory_kept_inside_dls_cache_names_no_owner_either() {
        // The last way the layout can be read into a path that is not one. This is
        // inside the cache, so dl already counts it as its own for `--purge`, and
        // its leaf is the workspace's id — both guards satisfied — yet two
        // components above the leaf sits the cache directory itself, so the
        // "owner" would be `devlaunch`, the name of dl's cache. An owner has to be
        // a directory dl put under the cache, not the cache.
        let workspaces = listed(&one(
            "myproject",
            r#"{"localFolder": "/home/dev/.cache/devlaunch/scratch/myproject"}"#,
        ));

        assert_eq!(offered(&workspaces, cache())[0].label, "- | myproject");
    }

    #[test]
    fn the_owner_column_is_padded_so_the_ids_line_up() {
        // Alignment is a fact about the list, not about one row: every owner is
        // drawn to the width of the widest, so the ids start in the same column and
        // the eye can run down them. The dash is padded like any other owner.
        let workspaces = listed(
            r#"[
                {"id": "one", "source": {"gitRepository": "github.com/blooop/devlaunch"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "two", "source": {"gitRepository": "github.com/kinisi-robotics/kinisi_ros"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "three", "source": {"localFolder": "/home/dev/myproject"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        assert_eq!(
            offered(&workspaces, cache())
                .iter()
                .map(|offer| offer.label.clone())
                .collect::<Vec<_>>(),
            [
                "blooop          | one",
                "kinisi-robotics | two",
                "-               | three",
            ]
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
            offered(&workspaces, cache())
                .iter()
                .map(|offer| offer.workspace_id.clone())
                .collect::<Vec<_>>(),
            ["zebra", "alpha"]
        );
    }

    #[test]
    fn an_empty_listing_offers_nothing_and_is_not_a_picker() {
        let none = listed("[]");

        assert!(offered(&none, cache()).is_empty());
        // And no terminal is opened to say so: nothing to pick from is answered
        // before anything is drawn, whichever arity asked.
        assert_eq!(pick(&none, Arity::One, cache()), Pick::NoWorkspaces);
        assert_eq!(pick(&none, Arity::Several, cache()), Pick::NoWorkspaces);
    }

    #[test]
    fn a_row_that_names_no_workspace_is_no_choice() {
        // Python's `ws_map.get(selected)`: a label the map has not got answers
        // `None`, and `None` is the help and exit 1.
        let offers = offered(&listed(&one("mine", r#"{"localFolder": "/p"}"#)), cache());

        assert_eq!(chosen(&offers, Vec::new()), Pick::Quit);
        assert_eq!(
            chosen(&offers, vec!["something else".to_owned()]),
            Pick::Quit
        );
    }
}
