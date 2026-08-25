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
//! draws `{owner} | {repo} | {ref}`. Both of the columns that went were answering
//! questions nobody standing at this picker is asking: `kind` reads `local` for
//! every workspace dl makes, since dl always hands devpod a path, and `detail` is
//! the clone directory dl chose and manages — a long, mechanically derived path
//! whose own last component is already the id.
//!
//! **What replaced the id is `<owner> | <repo> | <branch>`.** Two of those come out
//! of dl's own clone layout, `<cache>/repos/<owner>/<repo>/<id>` ([`owner_of`],
//! [`repo_of`]), and the third out of the clone's `HEAD` ([`head_branch_of`]) — no
//! records opened, no config read, one small file per row.
//!
//! **It is a table, headings and all.** Every cell is padded to its column's widest
//! entry, the heading over it counted as one of them ([`Columns`]), so a column
//! starts at the same place on every line and `OWNER | REPO | BRANCH` sits directly
//! over the cells it names. The heading is drawn in skim's header beside the
//! [`invitation`] rather than offered as a row, which is what keeps it from being
//! filtered away, marked with TAB, or picked. A column is named exactly when some
//! row has something in it: a listing of nothing dl cloned is headed `OWNER | REPO`
//! and stops.
//!
//! A workspace dl did not clone sits in that grid too — its devpod name in the repo
//! column, and the row ends there rather than drawing an empty branch cell. The name
//! used to run on through the space a repo and a branch would have taken, exempt
//! from the column and from the width it is measured for; it pays that width now,
//! which is the price of every row being on one grid.
//!
//! The owner is what an id cannot carry at all, so a fork and its upstream used to
//! be two rows spelled the same. The suffix is what an id carries that nobody reads:
//! four characters of hash, there to keep two branches from sharing a name.
//!
//! **The branch is read rather than reconstructed, and that is a change.** These
//! columns used to come from taking the id apart — strip the repo prefix, strip the
//! suffix, and slug what is left. That answered with a *slug*, so `feature/auth` and
//! `feature-auth` drew one row twice and a long branch drew short; and it worked at
//! all only because the suffix had a shape a parser could recognise, which four
//! characters of base 36 do not. `HEAD` is exact, and it says which branch is checked
//! out *now* rather than which one the workspace was made for.
//!
//! **The branch is never given up to keep two rows apart.** Where a split would
//! leave two rows drawn the same — two workspaces of one repository on one branch,
//! which is what the id-scheme migration leaves for a while — both gain a fourth
//! column holding their whole id, rather than collapsing into it. The row's own text
//! is what [`chosen`] maps back to a workspace, so *something* has to tell the two
//! apart (see [`named`]); what does not follow is that it has to be the branch that
//! goes. A picker is read one row at a time with the whole terminal width to spend,
//! and the branch is what somebody standing at it is choosing by — so the id is
//! appended and the branch stays where it was. This is the opposite trade from
//! [the terminal tab](devlaunch_core::flows::launch::TerminalTitle), which drops the
//! suffix and truncates the branch, and it is opposite because the constraints are:
//! a tab is a few characters read at a glance and needs no uniqueness at all.
//!
//! One thing this deliberately does not do: it does not draw `owner/repo@branch`.
//! That reads as a spec `dl` would accept, and this is a picker, not a place to
//! retype what you are already pointing at.
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
//! [`offered`] is that list and nothing else — a pure function of what devpod said
//! and where dl's cache is, which is what makes the spec testable without a
//! terminal. [`pick`] is the interactive half.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use devlaunch_core::clients::devpod::Workspace;
use devlaunch_core::domain::workspace_id::WorkspaceId;
use devlaunch_core::domain::workspace_state::NonEmpty;
use devlaunch_core::flows::listing::{head_branch_of, owner_of, repo_of};
use skim::prelude::*;

/// One row the picker offers, and the workspace it stands for.
///
/// Both halves together, because a label a user can pick that maps back to nothing
/// is a row that does nothing: Python keeps the same pair in its `ws_map`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Offer {
    pub(crate) label: String,
    /// The same row with the padding taken out: what this row *says*, as opposed to
    /// how wide it happened to be drawn.
    ///
    /// Kept because a pick is reported back to the user in words
    /// ([`render::picked`](crate::render::picked)), and `label` is padded against the
    /// widest entry in the list — quoting it would put runs of spaces inside a
    /// sentence. The padding is *internal*: [`label`] pads every column and then
    /// trims the line's end, so a label never ends in it and trimming one does not
    /// recover this. It has to be built, and it is built beside `label` from the
    /// same naming, which is the only thing that keeps the two from drifting.
    pub(crate) unpadded: String,
    pub(crate) workspace_id: String,
    /// The triple this row's clone is of, when the id it derives is this very
    /// workspace.
    ///
    /// The picker is the one caller that opens a workspace **by id** and still
    /// knows its triple: it read the owner and repo out of the cache layout and the
    /// branch out of the clone's `HEAD` to draw the row. A launch handed a bare id
    /// cannot recover any of that, so without this the tab of every workspace
    /// opened from the picker reads as the id
    /// (`devlaunch-main-3j1t`) where the same workspace opened as
    /// `dl blooop/devlaunch@main` reads `devlaunch@main`.
    ///
    /// `None` where the row is not one of dl's clones, and **also** where the
    /// recovered triple derives some *other* id: `HEAD` is the branch checked out
    /// now, so a `git switch` inside the container leaves a triple that is no
    /// longer this workspace's. Core makes that check itself
    /// (`flows::launch::titled`); this carries the evidence, not the verdict.
    pub(crate) triple: Option<WorkspaceId>,
}

/// What the owner column says for a workspace whose source names no owner: one
/// opened from a path or a URL that is not GitHub's.
///
/// A dash rather than an empty cell, because the column is padded — a blank there
/// reads as a column that failed to draw, where a dash reads as an answer.
const NO_OWNER: &str = "-";

/// One row's naming, before padding turns it into a label.
struct Naming {
    owner: String,
    tail: Tail,
    /// The triple this row's clone layout and `HEAD` say the workspace is, when
    /// they say one.
    ///
    /// Kept beside [`Self::tail`] rather than read back out of it, because the two
    /// answer different questions and stop agreeing: a row that collides is
    /// *drawn* as its whole id ([`named`]) while still *being* the triple. It is
    /// what a launch by bare id has no way to recover for itself, so it travels out
    /// on the [`Offer`] (see [`Offer::triple`]).
    triple: Option<WorkspaceId>,
    /// The workspace's own id, kept beside the tail because it is what every
    /// fallback falls back *to* — and a fallback that had to go looking for it
    /// again could look in the wrong row.
    id: String,
}

/// What a row says after its owner.
///
/// A sum and not a repo beside an optional ref, because the two are not
/// independently absent: dl's own layout names both halves or neither, and a row
/// holding one of them would be a column with the wrong thing under it.
enum Tail {
    /// dl's own clone: the repo out of the cache layout, and the branch out of the
    /// clone's `HEAD`.
    Split {
        repo: String,
        git_ref: String,
        /// The workspace id, drawn as a fourth column, when another row says the
        /// same `<repo> | <branch>` and this is what tells the two apart.
        ///
        /// `None` for nearly every row, and that is the shape worth keeping: the
        /// id is four columns of machinery for a question only a collision asks,
        /// so it is on screen exactly where it is doing work. Which rows collide
        /// is a fact about the whole list, so only [`named`] may set it.
        distinguished_by: Option<String>,
    },
    /// devpod's workspace name, whole — for a workspace dl did not clone, which
    /// has no layout to read a repo and no clone to read a `HEAD` from.
    Whole(String),
}

/// The whole picker: its heading line, and the rows under it.
///
/// One value rather than two returns, because the heading is only true of *these*
/// rows — it is padded to the widths they were drawn to, and it names a column
/// exactly when one of them has something in it. A heading built anywhere else, or
/// kept beside a list it was not measured from, is a heading that stops lining up
/// with what is under it.
pub(crate) struct Offering {
    /// The column headings, padded to the same widths as the rows.
    ///
    /// Drawn inside skim's header along with the [`invitation`], which is what keeps
    /// it out of the rows: a heading offered as a row is a row that can be filtered
    /// away, marked with TAB, and picked.
    pub(crate) heading: String,
    pub(crate) offers: Vec<Offer>,
}

/// Every workspace devpod listed, in devpod's order, as the picker shows it.
///
/// No filtering of any kind: the picker is a view of `dl --ls`, so a workspace
/// devlaunch did not create and one whose source it cannot read are both offered.
///
/// Every column is padded to the widest entry *in this list*, which is why the
/// labels are built here in one pass over all of them rather than one workspace at
/// a time: alignment is a fact about the set of rows, not about any one of them.
///
/// `cache_dir` is where dl keeps its clones, and it is what [`owner_of`],
/// [`repo_of`] and [`head_branch_of`] are gated on the same layout reading — the same directory
/// `--purge` decides ownership by, so they cannot disagree about which workspaces
/// are dl's.
pub(crate) fn offered(workspaces: &[Workspace], cache_dir: &Path) -> Offering {
    // One pass, and it is [`named`]'s: a key that is the row's own unpadded text
    // sees every way two rows can be drawn alike, including the cross-shape case a
    // whole-name row makes with a split one, and de-splits only the rows that
    // collided. There used to be a second, whole-list pass here for that case; it
    // put every split row in the listing back to its id over one ambiguous pair,
    // and it compared padded labels, so whether it fired at all depended on how
    // wide some unrelated third row was.
    //
    // What is still not promised is distinctness: two workspaces of one id in two
    // devpod contexts draw one row whatever is done here, because an id is unique
    // per context and nothing in an `Offer` carries the context. That predates the
    // columns — `<owner> | <id>` collided the same way — and closing it means
    // addressing a workspace by more than its id.
    let namings = named(workspaces, cache_dir);
    let columns = Columns::of(&namings);
    Offering {
        heading: heading(&columns),
        offers: workspaces
            .iter()
            .zip(&namings)
            .map(|(workspace, naming)| Offer {
                label: label(naming, &columns),
                triple: naming.triple.clone(),
                // The same row asked for at no width, which is what `shared_key`
                // asks `label` for and for the same reason: the row's own text,
                // with the widths of its neighbours taken out.
                unpadded: label(naming, &Columns::UNPADDED),
                workspace_id: workspace.id.clone(),
            })
            .collect(),
    }
}

/// How wide each column is drawn, and which of them this listing has anything to
/// put in.
///
/// Widths and not a per-row measurement, because alignment is a fact about the set
/// of rows: a column is as wide as its widest cell, and every row pays that width so
/// that whatever comes after it starts at the same place on every line.
///
/// **The heading counts as a cell.** A column whose entries are all narrower than
/// the word over them would otherwise have that word overhang into the next column,
/// which is a heading that lines up with nothing — [`render::table_lines`] reproduces
/// exactly that for `dl --ls`'s `WORKSPACE`, deliberately, because it is Python's and
/// the harness compares it byte for byte. Nothing here is anybody's byte-for-byte, so
/// it is measured the way it should have been.
///
/// [`render::table_lines`]: crate::render::table_lines
struct Columns {
    owner: usize,
    /// The repository — **or** the devpod name of a workspace dl did not clone,
    /// which is drawn in this column too and therefore measured for it.
    ///
    /// The name used to be exempt: it ran on through the space a repo and a branch
    /// would have taken, so a long one cost the rows around it nothing. It sits in
    /// the column now, which is what puts every row on the same grid, and the price
    /// is that one long foreign name widens the column for everybody — the same
    /// price one long repository name has always cost.
    repo: usize,
    /// `None` when no row in this listing has a branch: nothing dl cloned is in it,
    /// so there is no column and no heading over it.
    git_ref: Option<usize>,
    /// Whether any row was drawn its whole id to tell it from another
    /// ([`Tail::Split::distinguished_by`]).
    ///
    /// A flag and not a width, because it is the last column: nothing follows it, so
    /// there is nothing to line up and the only question left is whether the heading
    /// names a column that exists.
    workspace: bool,
}

impl Columns {
    /// Every column at no width, which is what [`shared_key`] measures a row by: the
    /// row's own text, with the widths of its neighbours taken out.
    const UNPADDED: Self = Self {
        owner: 0,
        repo: 0,
        git_ref: Some(0),
        workspace: false,
    };

    fn of(namings: &[Naming]) -> Self {
        let cells =
            |heading: &'static str, texts: Vec<&str>| widest(std::iter::once(heading).chain(texts));
        let branches: Vec<&str> = namings
            .iter()
            .filter_map(|naming| match &naming.tail {
                Tail::Split { git_ref, .. } => Some(git_ref.as_str()),
                Tail::Whole(_) => None,
            })
            .collect();
        Self {
            owner: cells(
                OWNER,
                namings.iter().map(|naming| naming.owner.as_str()).collect(),
            ),
            repo: cells(
                REPO,
                namings
                    .iter()
                    .map(|naming| match &naming.tail {
                        Tail::Split { repo, .. } => repo.as_str(),
                        Tail::Whole(name) => name.as_str(),
                    })
                    .collect(),
            ),
            git_ref: (!branches.is_empty()).then(|| cells(BRANCH, branches)),
            workspace: namings.iter().any(|naming| {
                matches!(
                    &naming.tail,
                    Tail::Split {
                        distinguished_by: Some(_),
                        ..
                    }
                )
            }),
        }
    }
}

/// The words over the columns.
const OWNER: &str = "OWNER";
const REPO: &str = "REPO";
const BRANCH: &str = "BRANCH";
const WORKSPACE: &str = "WORKSPACE";

/// The heading line, padded to the same widths the rows were drawn to.
///
/// A column is named exactly when some row has something in it, so a listing of
/// nothing but workspaces dl did not clone is headed `OWNER | REPO` and stops —
/// there is no branch to head, and no id column, so naming either would put a word
/// over empty air.
///
/// **`REPO` heads the devpod name of a foreign workspace too**, because that is the
/// column the name is drawn in. It is the one place the heading is looser than the
/// cells under it: a workspace dl did not clone has no repository, and what it has
/// instead is its own name. The alternative — a fifth shape where the name goes in
/// the `WORKSPACE` column — draws two empty cells to get there and puts the only
/// thing that row says furthest right on the line.
fn heading(columns: &Columns) -> String {
    let mut line = format!(
        "{OWNER:owner$} | {REPO:repo$}",
        owner = columns.owner,
        repo = columns.repo,
    );
    if let Some(width) = columns.git_ref {
        line.push_str(&format!(" | {BRANCH:width$}"));
        if columns.workspace {
            line.push_str(&format!(" | {WORKSPACE}"));
        }
    }
    trimmed(line)
}

/// How every row wants to be named, with a column added to any that would not
/// otherwise have been unique.
///
/// **The second pass is not cosmetic.** [`chosen`] maps a picked row back to its
/// workspace by the row's own text, first match winning, so two rows drawn the same
/// are a row that selects the other workspace — and `dl rm` is one of the verbs this
/// picker opens for. Two workspaces of one repository on one branch really do
/// happen: the id-scheme migration leaves the renamed clone under its derived id
/// beside a container still carrying the old one, until `dl --reconcile` or a
/// `recreate` catches up.
///
/// The whole id is what settles it, because that is the string the ids were given a
/// hashed suffix to make unique in the first place. It is **appended**, in a fourth
/// column, rather than replacing the split: the two rows have the same branch, so
/// the branch is not what was ambiguous, and taking it off screen to fix an
/// ambiguity it did not cause leaves a person picking between two ids. The branch is
/// what the picker is read by; the id is the tiebreak, and it is drawn in exactly
/// the case it is doing work.
///
/// **Rounds, not a pass, because an append is a new string.** A row that gains its
/// id says something no row said before, and the list has to be asked about *that*:
/// rows one and two colliding can leave row one drawn exactly like a row three that
/// was never in their group, and a single pass never revisits row three. Collapsing
/// to the whole id could not do this — `owner | id` is unique by construction, since
/// ids are unique within a devpod context and nothing else is drawn in that shape —
/// so appending is what gives that guarantee up, and the rounds are what buy it back.
///
/// A row that still collides *after* saying its id falls all the way back to
/// `owner | id`, and that is where it stops. So a row moves at most twice, the moves
/// only ever go one way, and the round that moves nothing is the fixpoint. Reaching
/// the fallback at all takes a devpod workspace whose id holds ` | `, which devpod
/// itself will not create. Correctness first where it does: `dl rm` acting on the
/// wrong row is one workspace deleted in place of another, where a bare id is only a
/// row that is harder to read.
///
/// (Two contexts holding one id draw one row whatever is done here — see
/// [`offered`].)
fn named(workspaces: &[Workspace], cache_dir: &Path) -> Vec<Naming> {
    let mut namings: Vec<Naming> = workspaces
        .iter()
        .map(|workspace| {
            let owner = owner_of(workspace, cache_dir).unwrap_or_else(|| NO_OWNER.to_owned());
            let tail = match (
                repo_of(workspace, cache_dir),
                head_branch_of(workspace, cache_dir),
            ) {
                // Both or neither: the two are one reading of one layout, and a repo
                // with no ref beside it would be a column with the wrong thing under
                // it. `listing` answers them separately only so that neither has to
                // return a pair.
                (Some(repo), Some(git_ref)) => Tail::Split {
                    repo,
                    git_ref,
                    distinguished_by: None,
                },
                _ => Tail::Whole(workspace.id.clone()),
            };
            let triple = match &tail {
                Tail::Split { repo, git_ref, .. } => WorkspaceId::new(&owner, repo, git_ref).ok(),
                Tail::Whole(_) => None,
            };
            Naming {
                owner,
                tail,
                triple,
                id: workspace.id.clone(),
            }
        })
        .collect();
    loop {
        let drawn = tallied(&namings);
        let mut moved = false;
        for naming in &mut namings {
            if drawn[&shared_key(naming)] <= 1 {
                continue;
            }
            let id = naming.id.clone();
            match &mut naming.tail {
                Tail::Split {
                    distinguished_by: slot @ None,
                    ..
                } => {
                    *slot = Some(id);
                    moved = true;
                }
                Tail::Split { .. } => {
                    naming.tail = Tail::Whole(id);
                    moved = true;
                }
                // `owner | id` is where a row stops. A second row drawn like this
                // one shares its owner *and* its id, which is one workspace listed
                // under two devpod contexts and not something a label can separate.
                Tail::Whole(_) => {}
            }
        }
        if !moved {
            break;
        }
    }
    namings
}

/// How many rows say each thing.
fn tallied(namings: &[Naming]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for naming in namings {
        *counts.entry(shared_key(naming)).or_default() += 1;
    }
    counts
}

/// What two rows drawn the same have in common: the text of the row itself.
///
/// The padding a label is drawn with is deliberately not in the key. A collision is
/// between what two rows *say*, not how wide they happened to be printed — and
/// padding is a fact about the widest row in the list, so a key that carried it
/// would make "are these two rows the same?" depend on a third row that has nothing
/// to do with either of them.
fn shared_key(naming: &Naming) -> String {
    // The unpadded label, which is to say: the row's own text, with the widths of
    // its neighbours taken out. Two rows collide when they *say* the same thing, and
    // a key built from the fields separately cannot see the collision a two-column
    // row makes with a three-column one — `<owner> | <name>` equals
    // `<owner> | <repo> | <ref>` when the name is those last two columns, while the
    // keys `owner\0name` and `owner\0repo\0ref` differ.
    //
    // Asking `label` rather than restating its format is the point: the separator,
    // the column count and which fields are drawn are decided in exactly one place,
    // so a change to how a row is drawn cannot leave the key describing the old
    // drawing. It also makes the NUL delimiter unnecessary — ` | ` is the delimiter,
    // and it is the one on screen.
    label(naming, &Columns::UNPADDED)
}

/// One row, padded into the label skim draws and [`chosen`] reads back.
///
/// A split row takes three columns, four where a collision made it say its id. A
/// whole-name row takes two: the devpod name sits in the repo column, and the row
/// stops there rather than drawing an empty branch cell after it. Every cell is
/// padded to its column, so the fourth column starts at the same place on every line
/// that has one and the [`heading`] over them all lines up.
///
/// **And then the line's end is trimmed**, which is what keeps the padding
/// *internal*. A row's last cell has nothing after it to line up with, so the spaces
/// that would follow it are invisible on a terminal and merely wrong everywhere else:
/// in [`Offer::unpadded`], in a `dl rm` receipt quoting the row, and in a test
/// asserting what was drawn.
///
/// The branch is drawn whole in every shape that has one. Nothing about the picker
/// is short of room the way a tab bar is: rows are read one at a time, down the
/// terminal, and a branch that ran past the width would still be searchable by the
/// characters skim never drew.
///
/// Nothing here decides which rows collide. That is a fact about the whole list, and
/// [`named`] is where it is established; this only draws what it was told.
fn label(naming: &Naming, columns: &Columns) -> String {
    let owner_width = columns.owner;
    let repo_width = columns.repo;
    // A split row always has a branch, so the column exists whenever one is drawn;
    // `None` means no split row is in this listing, and nothing below runs.
    let ref_width = columns.git_ref.unwrap_or(0);
    trimmed(match &naming.tail {
        Tail::Split {
            repo,
            git_ref,
            distinguished_by,
        } => {
            let tiebreak = match distinguished_by {
                Some(id) => format!(" | {id}"),
                None => String::new(),
            };
            format!(
                "{owner:owner_width$} | {repo:repo_width$} | {git_ref:ref_width$}{tiebreak}",
                owner = naming.owner
            )
        }
        Tail::Whole(name) => format!(
            "{owner:owner_width$} | {name:repo_width$}",
            owner = naming.owner
        ),
    })
}

/// A drawn line with the padding its last cell did not need taken off.
///
/// One function because [`label`] and [`heading`] both owe it, and a heading that
/// kept trailing spaces where the rows under it did not would be a line that
/// compares unequal to itself for a reason nobody can see.
fn trimmed(line: String) -> String {
    let end = line.trim_end().len();
    let mut line = line;
    line.truncate(end);
    line
}

/// The width of the widest of *texts*, in the characters a terminal draws and
/// `{:width$}` counts — not the bytes a non-ASCII owner or repo would measure.
fn widest<'a>(texts: impl Iterator<Item = &'a str>) -> usize {
    texts.map(|text| text.chars().count()).max().unwrap_or(0)
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

/// One row the picker took: the workspace it names, and the words it was chosen by.
///
/// The pair rather than the id alone, because the two answer different questions and
/// only one of them is on screen at the moment of choosing. An id is
/// `<repo-slug>-<ref-slug>-<suffix>` ([`devlaunch_core::domain::workspace_id`]) and
/// carries **no owner**, so `blooop | devlaunch | main` and
/// `myfork | devlaunch | main` are two rows whose ids differ only in the hashed
/// suffix this picker deliberately never draws. A command that reported the id alone
/// would be telling the user about a workspace they cannot check against the row they
/// took; `row` is what closes that, and [`render::picked`](crate::render::picked) is
/// where the two are printed together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chosen {
    pub(crate) workspace_id: String,
    /// The row's own text, unpadded — [`Offer::unpadded`], not [`Offer::label`].
    pub(crate) row: String,
    /// [`Offer::triple`] for the row that was taken: what this workspace is, for a
    /// launch that is about to be handed only what it is *called*.
    pub(crate) triple: Option<WorkspaceId>,
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
    /// These workspaces, in the order skim handed the rows back. One entry always
    /// under [`Arity::One`]; [`NonEmpty`] because an empty set of choices is
    /// [`Pick::Quit`], not a batch of nothing.
    Chose(NonEmpty<Chosen>),
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
    let offering = offered(workspaces, cache_dir);
    if offering.offers.is_empty() {
        return Pick::NoWorkspaces;
    }
    // Asked before skim is entered, and not as politeness: skim opens `/dev/tty`
    // and unwraps the result, so a run without one would abort the process rather
    // than answer.
    if !a_terminal_exists() {
        return Pick::NoTerminal;
    }
    let rows = run_skim(&offering, arity);
    chosen(&offering.offers, rows)
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
fn skim_options(arity: Arity, heading: &str) -> SkimOptions {
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
        //
        // Two lines, and skim draws them as two: `Header::with_options` splits the
        // string on newlines and gives each its own row, at the same column the item
        // rows are printed at (`src/header.rs`, `src/selection.rs` — both `col(2)`),
        // which is what lets the headings sit directly over the cells they name.
        // Both belong here rather than one of them above the picker, for the reason
        // the paragraph above gives: skim's alternate screen covers anything printed
        // on the way in.
        header: Some(format!("{}\n{heading}", invitation(arity))),
        ..Default::default()
    }
}

/// The rows skim was left on: empty when the picker was quit without an answer.
fn run_skim(offering: &Offering, arity: Arity) -> Vec<String> {
    let options = skim_options(arity, &offering.heading);
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for row in rows_of(&offering.offers) {
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
            .map(|offer| Chosen {
                workspace_id: offer.workspace_id.clone(),
                row: offer.unpadded.clone(),
                triple: offer.triple.clone(),
            })
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
    //! The interactive half needs a terminal, so what these can reach of it is the
    //! options `dl` asks for and nothing further. `tests/picker.rs` takes it from
    //! there: it opens a pty, runs the binary on it and reads the screen back, which
    //! is the only seam that can tell an option that is spelled right from one that
    //! draws something.

    use super::*;

    use devlaunch_core::flows::listing::CommandContext;
    use devlaunch_test_support::{FakeRunner, Response};

    /// Where dl keeps its clones in these tests: the owner column is read out of
    /// the layout under it, and out of nothing under any other directory.
    ///
    /// **A real directory, and it did not have to be until the ref column moved.**
    /// The owner and the repo are still read off the path and need nothing on disk,
    /// but the branch is read out of the clone's own `HEAD`
    /// ([`head_branch_of`]), so a row that names a clone which is not there has no
    /// branch to show.
    ///
    /// **One per test, and that is the point rather than an accident.** A cache
    /// shared by the module raced: two tests calling [`Self::clone_at`] on one path
    /// both write `HEAD`, `std::fs::write` truncates before it writes, and the
    /// reader in between sees an empty file and a row with no branch. It surfaced as
    /// a *different* test failing intermittently, which is the worst way to find it.
    struct Cache(tempfile::TempDir);

    impl Cache {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("a cache directory"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        /// The workspaces devpod lists, from the JSON it would have printed.
        ///
        /// `{CACHE}` in *listing* stands for this cache, which is a real directory
        /// and therefore has a name only known at run time.
        fn listed(&self, listing: &str) -> Vec<Workspace> {
            let listing = listing.replace("{CACHE}", &self.path().display().to_string());
            let runner = FakeRunner::new().with_script(
                ["devpod", "list", "--output", "json"],
                Response::stdout(&listing),
            );
            CommandContext::new(&runner)
                .workspaces()
                .expect("a listing")
        }

        /// A clone of dl's own with *branch* checked out, at the layout `dl` puts it
        /// in.
        ///
        /// Writes the one line of `HEAD` that git would, which is all the ref column
        /// reads. Returns nothing: the listing names the same path through `{CACHE}`
        /// in [`Self::listed`], and having this return one would mean two spellings
        /// of it to keep in step.
        fn clone_at(&self, owner: &str, repo: &str, id: &str, branch: &str) {
            let git = self
                .path()
                .join("repos")
                .join(owner)
                .join(repo)
                .join(id)
                .join(".git");
            std::fs::create_dir_all(&git).expect("a clone");
            std::fs::write(git.join("HEAD"), format!("ref: refs/heads/{branch}\n"))
                .expect("a HEAD");
        }
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
        let cache = Cache::new();
        let workspaces = cache.listed(
            r#"[
                {"id": "mine", "source": {"localFolder": "/home/dev/myproject"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "from-an-image", "source": {"image": "ubuntu:24.04"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offers = offered(&workspaces, cache.path()).offers;

        assert_eq!(
            offers.iter().map(|offer| &offer.label).collect::<Vec<_>>(),
            ["-     | mine", "-     | from-an-image"]
        );
        // Picking the row maps back to the workspace, which is what makes it an
        // offer rather than a line of text.
        assert_eq!(
            chosen(&offers, vec![offers[1].label.clone()]),
            Pick::Chose(one_id("- | from-an-image", "from-an-image"))
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
        assert_eq!(skim_options(Arity::One, A_HEADING).layout, "reverse");
        assert_eq!(skim_options(Arity::Several, A_HEADING).layout, "reverse");
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
        let asked = skim_options(Arity::One, A_HEADING);

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
        //
        // The column headings share the header with it, on a second line: skim
        // splits this string on newlines and draws one row each, so the invitation
        // stays the sentence above the table and the headings stay the line above
        // the rows.
        assert_eq!(
            skim_options(Arity::Several, "OWNER  | REPO")
                .header
                .as_deref(),
            Some(
                "Select workspaces (type to filter, TAB to mark several):\n\
                 OWNER  | REPO"
            )
        );
        // And no TAB clause on the picker that takes one row: a key named in the
        // header and inert under the cursor is worse than an unmentioned one.
        assert_eq!(
            skim_options(Arity::One, "OWNER  | REPO").header.as_deref(),
            Some(
                "Select workspace (type to filter):\n\
                 OWNER  | REPO"
            )
        );
    }

    /// A heading for the tests that are not about one: what the picker draws over
    /// its columns is [`heading`]'s business, and these are asking about the levers
    /// beside it.
    const A_HEADING: &str = "OWNER | REPO";

    #[test]
    fn what_the_picker_will_take_is_the_arity_and_the_order_is_never_skim_s() {
        // The two options the layout change sits beside, so a mistyped struct
        // literal cannot trade one for another unnoticed.
        assert!(!skim_options(Arity::One, A_HEADING).multi);
        assert!(skim_options(Arity::Several, A_HEADING).multi);
        assert!(skim_options(Arity::One, A_HEADING).no_sort);
        assert!(skim_options(Arity::Several, A_HEADING).no_sort);
    }

    /// A `Pick::Chose` of exactly these `(row, id)` pairs, for the assertions below.
    ///
    /// Both halves are written out rather than derived from the offers under test: an
    /// expectation built by asking the same `offered` call what it said would agree
    /// with itself whatever it answered, and the row is now part of what a pick
    /// promises.
    fn ids(named: &[(&str, &str)]) -> Pick {
        Pick::Chose(
            NonEmpty::of(named.iter().map(|(row, id)| Chosen {
                workspace_id: (*id).to_owned(),
                row: (*row).to_owned(),
                triple: None,
            }))
            .expect("at least one id"),
        )
    }

    /// A pick of one row, for a workspace whose triple the picker could not
    /// recover: a foreign one, or one whose clone is gone.
    fn one_id(row: &str, named: &str) -> NonEmpty<Chosen> {
        one_pick(row, named, None)
    }

    /// A pick of one row of dl's own, which carries the triple its clone said it is.
    fn one_clone(row: &str, named: &str, triple: (&str, &str, &str)) -> NonEmpty<Chosen> {
        let (owner, repo, branch) = triple;
        one_pick(
            row,
            named,
            Some(WorkspaceId::new(owner, repo, branch).expect("a safe triple")),
        )
    }

    fn one_pick(row: &str, named: &str, triple: Option<WorkspaceId>) -> NonEmpty<Chosen> {
        NonEmpty::of([Chosen {
            workspace_id: named.to_owned(),
            row: row.to_owned(),
            triple,
        }])
        .expect("one id")
    }

    #[test]
    fn every_row_carries_its_own_index_or_marking_cannot_accumulate() {
        // skim's multi-select keys each marked row by `(run, get_index())`, and the
        // trait's `get_index()` defaults to 0. Rows all answering 0 therefore share
        // one key, and every TAB after the first *removes* the previous mark
        // instead of adding to it — observed live: mark two workspaces, and only
        // the last one is acted on. Distinct indices are what make marking
        // accumulate, so they are the spec.
        let cache = Cache::new();
        let offers = offered(
            &cache.listed(
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
            cache.path(),
        )
        .offers;

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
        let cache = Cache::new();
        let offers = offered(
            &cache.listed(
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
            cache.path(),
        )
        .offers;

        assert_eq!(
            chosen(
                &offers,
                vec![offers[2].label.clone(), offers[0].label.clone()]
            ),
            ids(&[("- | third", "third"), ("- | first", "first")])
        );
        // A row naming no workspace is dropped rather than sinking the rows that
        // do name one — the batch the user marked still happens.
        assert_eq!(
            chosen(
                &offers,
                vec!["something else".to_owned(), offers[1].label.clone()]
            ),
            ids(&[("- | second", "second")])
        );
    }

    #[test]
    fn a_git_source_is_offered_under_the_owner_its_url_names() {
        let cache = Cache::new();
        let workspaces = cache.listed(&one(
            "wf",
            r#"{"gitRepository": "https://github.com/blooop/devlaunch.git"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache.path()).offers[0].label,
            "blooop | wf"
        );
    }

    #[test]
    fn a_clone_of_dls_own_is_read_apart_into_owner_repo_and_ref() {
        // The row a user of `dl` actually sees: every workspace `dl owner/repo`
        // makes is a clone at `<cache>/repos/<owner>/<repo>/<workspace id>` handed
        // to devpod as a path, so the owner and the repo are read back out of the
        // layout and the ref off the id — no records opened, no config read, no disk
        // touched. The four-character suffix does not appear: it is what keeps two
        // branches from sharing an id, and choosing a workspace never involves
        // reading it.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        let workspaces = cache.listed(&one(
            "devlaunch-main-3j1t",
            r#"{"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache.path()).offers[0].label,
            "blooop | devlaunch | main"
        );
    }

    #[test]
    fn a_repo_whose_slug_the_id_cut_is_still_read_apart() {
        // The id's repo part is cut to twenty characters when the id would overflow,
        // so the prefix in the id is not the repo directory's name. The *directory*
        // is what the column shows, because that is the repository's actual name and
        // the cut is an artefact of the id's length budget.
        let cache = Cache::new();
        cache.clone_at(
            "blooop",
            "an-extraordinarily-long-repository-name-indeed",
            "an-extraordinarily-l-main-pd3w",
            "main",
        );
        let workspaces = cache.listed(&one(
            "an-extraordinarily-l-main-pd3w",
            r#"{"localFolder": "{CACHE}/repos/blooop/an-extraordinarily-long-repository-name-indeed/an-extraordinarily-l-main-pd3w"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache.path()).offers[0].label,
            "blooop | an-extraordinarily-long-repository-name-indeed | main"
        );
    }

    #[test]
    fn two_rows_that_would_be_drawn_alike_say_their_ids_and_keep_their_branch() {
        // Two workspaces of one repository on one branch: two ids, two clones, and
        // one thing to say about either. Drawn apart they would be the same row
        // twice, and `chosen` maps a picked row back by its text with the first
        // match winning, so marking the second would remove the first. `dl rm` is
        // one of the verbs this picker opens for, so that is a workspace deleted for
        // a legibility win.
        //
        // This is not a contrived pair. The id-scheme migration leaves exactly it:
        // the renamed clone under its derived id, and the container still carrying
        // the old one until `dl --reconcile` or a `recreate` catches up.
        //
        // Both rows gain the column, not just the second, because there is no first:
        // the collision is between what the two rows say, and neither has a better
        // claim on the shorter spelling.
        //
        // **The branch stays.** These rows used to collapse into their whole ids,
        // which fixed the ambiguity by deleting the column that was never ambiguous
        // -- both are on `main` -- and left a person picking between two ids. The id
        // is appended instead: same guarantee, and the branch is still what the row
        // is read by.
        //
        // The pair this used to use was `feature/auth` beside `feature-auth`, which
        // collided because the ref was recovered by slugging an id and `slug`
        // collapses `/` and `-` alike. The ref is read from the clone's `HEAD` now,
        // so those two spell themselves apart and are no longer a collision at all.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-legacy", "main");
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-main-legacy",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-legacy"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offers = offered(&workspaces, cache.path()).offers;

        assert_eq!(
            offers.iter().map(|offer| &offer.label).collect::<Vec<_>>(),
            [
                // `main` is padded to `BRANCH`, the widest thing in that column, so
                // the two ids start in the same place and read as a column.
                "blooop | devlaunch | main   | devlaunch-main-3j1t",
                "blooop | devlaunch | main   | devlaunch-main-legacy",
            ]
        );
        // And the property the fallback exists for: each row still maps back to its
        // own workspace.
        assert_eq!(
            chosen(&offers, vec![offers[1].label.clone()]),
            Pick::Chose(one_clone(
                "blooop | devlaunch | main | devlaunch-main-legacy",
                "devlaunch-main-legacy",
                ("blooop", "devlaunch", "main"),
            ))
        );
    }

    #[test]
    fn a_split_row_cannot_be_drawn_the_same_as_a_whole_name_row() {
        // The cross-shape collision the two-column and three-column rows can make
        // between them: a workspace dl did not clone keeps whatever name devpod has
        // for it, and if that name is the middle and right columns of some *other*
        // row, the two rows are the same text. `chosen` then maps both to whichever
        // came first, so picking the second one acts on the first -- and `dl rm` is
        // one of the verbs this picker opens for.
        //
        // The collision key cannot see this one: the two rows have different key
        // shapes, so counting keys finds no duplicate.
        let cache = Cache::new();
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch | main",
                 "source": {"gitRepository": "https://github.com/blooop/devlaunch.git"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offers = offered(&workspaces, cache.path()).offers;

        let labels: Vec<&String> = offers.iter().map(|offer| &offer.label).collect();
        assert_ne!(labels[0], labels[1], "two rows drawn alike: {labels:?}");
        // And the property that matters: each row still reaches its own workspace.
        assert_eq!(
            chosen(&offers, vec![offers[1].label.clone()]),
            Pick::Chose(one_id("blooop | devlaunch | main", "devlaunch | main"))
        );
    }

    #[test]
    fn a_wider_third_row_cannot_hide_the_cross_shape_collision() {
        // The regression `a_split_row_cannot_be_drawn_the_same_as_a_whole_name_row`
        // pins, made invisible to the guard by a row that has nothing to do with
        // either of them.
        //
        // `all_distinct` was asked of the *padded* labels, and padding is a fact
        // about the widest row in the list. So a third workspace whose repo is one
        // character wider than `devlaunch` widens the repo column, the split row
        // gains one trailing space that the two-column whole-name row does not, the
        // two labels stop being equal *as strings* -- and the guard, which is
        // looking for equal strings, does not fire.
        //
        // What is on screen is then two rows one space apart, which is the
        // collision the guard exists to prevent: a person cannot tell them apart,
        // and `dl rm` is one of the verbs that opens this picker.
        let cache = Cache::new();
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch | main",
                 "source": {"gitRepository": "https://github.com/blooop/devlaunch.git"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "wayfinderx-main-0ei3",
                 "source": {"localFolder": "{CACHE}/repos/blooop/wayfinderx/wayfinderx-main-0ei3"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offers = offered(&workspaces, cache.path()).offers;
        let labels: Vec<String> = offers.iter().map(|offer| offer.label.clone()).collect();

        // Not `assert_ne!` on the raw strings: they differ by the padding, which is
        // exactly the difference a person cannot see. Two rows that are the same
        // once the padding is taken out are two rows drawn alike.
        let squashed: Vec<String> = labels
            .iter()
            .map(|label| label.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        assert_ne!(
            squashed[0], squashed[1],
            "two rows a person cannot tell apart: {labels:?}"
        );
    }

    #[test]
    fn a_cross_shape_collision_also_only_pulls_in_the_rows_that_collide() {
        // The scoping `a_collision_only_pulls_in_the_rows_that_collide` pins for two
        // split rows, held across the shape boundary too. The whole-name row and the
        // `main` split row are drawn alike; the `feature/auth` row of the same
        // repository is not, and keeps its columns.
        //
        // The pass this replaced could not do this: it de-split every row in the
        // listing, so one ambiguous pair put the id back on rows that were never
        // ambiguous.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at(
            "blooop",
            "devlaunch",
            "devlaunch-feature-auth-np10",
            "feature/auth",
        );
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch | main",
                 "source": {"gitRepository": "https://github.com/blooop/devlaunch.git"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-feature-auth-np10",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-feature-auth-np10"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        assert_eq!(
            offered(&workspaces, cache.path())
                .offers
                .iter()
                .map(|offer| offer.label.clone())
                .collect::<Vec<_>>(),
            [
                // Collided with the whole-name row, so it says its id too -- and
                // still says its branch, which is what the row is picked by.
                "blooop | devlaunch        | main         | devlaunch-main-3j1t",
                // The whole-name row is the widest thing in the repo column, which
                // is what every split row above and below it is now padded to.
                "blooop | devlaunch | main",
                // Did not collide with anything, so it keeps its columns -- and it
                // spells the ref, where the id could only offer `feature-auth`.
                "blooop | devlaunch        | feature/auth",
            ]
        );
    }

    #[test]
    fn a_row_that_appends_its_id_cannot_land_on_a_row_that_was_never_colliding() {
        // The append is a *new* string, so it has to be checked against the whole
        // list and not only against the group it came out of. Here rows one and two
        // collide, row one appends its id to settle that, and the row it becomes is
        // byte-identical to row three -- which was never in the colliding group and
        // is never revisited by a single pass.
        //
        // `chosen` is first-match-wins, so picking row three returned row one's
        // workspace. `dl rm` is one of the verbs this picker opens for, so that is
        // one workspace deleted in place of another.
        //
        // Collapsing to the whole id could not do this: `owner | id` is unique by
        // construction, since ids are unique within a devpod context and nothing
        // else is ever drawn in that shape. Appending gives that up, so the second
        // round is what buys it back.
        //
        // The foreign ids here hold ` | `, which devpod itself would not create --
        // the same synthetic `a_split_row_cannot_be_drawn_the_same_as_a_whole_name_row`
        // uses, and for the same reason: this picker draws whatever devpod lists.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch | main",
                 "source": {"gitRepository": "https://github.com/blooop/devlaunch.git"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch | main | devlaunch-main-3j1t",
                 "source": {"gitRepository": "https://github.com/blooop/devlaunch.git"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offers = offered(&workspaces, cache.path()).offers;
        let unpadded: Vec<&str> = offers.iter().map(|offer| offer.unpadded.as_str()).collect();

        assert_eq!(
            unpadded.len(),
            unpadded
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "two rows say the same thing: {unpadded:?}"
        );
        // And the row that says row three's text still maps to row three.
        assert_eq!(
            chosen(&offers, vec![offers[2].label.clone()]),
            Pick::Chose(one_id(
                &offers[2].unpadded,
                "devlaunch | main | devlaunch-main-3j1t",
            ))
        );
    }

    #[test]
    fn a_collision_only_pulls_in_the_rows_that_collide() {
        // The fallback is scoped to the rows drawn alike. A third workspace of the
        // same repository keeps its three columns, because nothing else is drawn
        // like it -- so one ambiguous pair does not put the id on a whole listing.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-legacy", "main");
        cache.clone_at(
            "blooop",
            "devlaunch",
            "devlaunch-feature-auth-np10",
            "feature/auth",
        );
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-main-legacy",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-legacy"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-feature-auth-np10",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-feature-auth-np10"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        assert_eq!(
            offered(&workspaces, cache.path())
                .offers
                .iter()
                .map(|offer| offer.label.clone())
                .collect::<Vec<_>>(),
            [
                "blooop | devlaunch | main         | devlaunch-main-3j1t",
                "blooop | devlaunch | main         | devlaunch-main-legacy",
                // The row that did not collide is what the branch column is widest
                // for, and the two ids above it line up on that width.
                "blooop | devlaunch | feature/auth",
            ]
        );
    }

    #[test]
    fn the_same_repo_name_under_two_owners_is_two_rows_that_read_apart() {
        // A fork and its upstream: one repository name, one branch, two workspaces.
        // The ids differ only in the suffix that is no longer drawn, so the owner
        // column is the whole of what tells the rows apart — and it is enough, so
        // neither row falls back.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at("someone", "devlaunch", "devlaunch-main-div6", "main");
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-main-div6",
                 "source": {"localFolder": "{CACHE}/repos/someone/devlaunch/devlaunch-main-div6"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        assert_eq!(
            offered(&workspaces, cache.path())
                .offers
                .iter()
                .map(|offer| offer.label.clone())
                .collect::<Vec<_>>(),
            ["blooop  | devlaunch | main", "someone | devlaunch | main"]
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
        let cache = Cache::new();
        let workspaces = cache.listed(&one(
            "myproject",
            r#"{"localFolder": "/home/dev/myproject"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache.path()).offers[0].label,
            "-     | myproject"
        );
    }

    #[test]
    fn a_directory_under_the_cache_that_is_not_shaped_like_a_clone_names_no_owner() {
        // The other half of the rule. Inside the cache, but its leaf is not this
        // workspace's id — so it is not the `<owner>/<repo>/<workspace id>` layout
        // dl writes, and there is no owner to be read out of it.
        let cache = Cache::new();
        let workspaces = cache.listed(&one(
            "mine",
            r#"{"localFolder": "{CACHE}/repos/blooop/devlaunch/somewhere-else"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache.path()).offers[0].label,
            "-     | mine"
        );
    }

    #[test]
    fn a_directory_kept_inside_dls_cache_names_no_owner_either() {
        // The last way the layout can be read into a path that is not one. This is
        // inside the cache, so dl already counts it as its own for `--purge`, and
        // its leaf is the workspace's id — both guards satisfied — yet two
        // components above the leaf sits the cache directory itself, so the
        // "owner" would be `devlaunch`, the name of dl's cache. An owner has to be
        // a directory dl put under the cache, not the cache.
        let cache = Cache::new();
        let workspaces = cache.listed(&one(
            "myproject",
            r#"{"localFolder": "{CACHE}/scratch/myproject"}"#,
        ));

        assert_eq!(
            offered(&workspaces, cache.path()).offers[0].label,
            "-     | myproject"
        );
    }

    #[test]
    fn the_repo_column_is_measured_over_the_foreign_names_drawn_in_it_too() {
        // Alignment across the two row shapes, and the price of putting them on one
        // grid. A workspace dl did not clone keeps devpod's name for it, and that
        // name is drawn in the repo column — so it is measured for the repo column,
        // and a long one widens it for every row.
        //
        // It used to be exempt: the column was measured over the split rows only and
        // the name ran on through the space a repo and a branch would have taken.
        // That kept two short rows short at the cost of the third starting its text
        // somewhere no other row did, under a heading naming a column it was not in.
        // A table is the thing being asked for, and this is what one costs.
        //
        // `kinisi_ros` also pins the column on the *directory* rather than the id's
        // prefix: the id spells it `kinisi-ros`, because `slug` turns `_` into `-`,
        // and the underscore is the repository's real name.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at(
            "kinisi-robotics",
            "kinisi_ros",
            "kinisi-ros-main-uwq5",
            "main",
        );
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "kinisi-ros-main-uwq5",
                 "source": {"localFolder": "{CACHE}/repos/kinisi-robotics/kinisi_ros/kinisi-ros-main-uwq5"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "a-very-long-workspace-name-of-its-own",
                 "source": {"localFolder": "/home/dev/myproject"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        assert_eq!(
            offered(&workspaces, cache.path())
                .offers
                .iter()
                .map(|offer| offer.label.clone())
                .collect::<Vec<_>>(),
            [
                "blooop          | devlaunch                             | main",
                "kinisi-robotics | kinisi_ros                            | main",
                "-               | a-very-long-workspace-name-of-its-own",
            ]
        );
    }

    #[test]
    fn the_owner_column_is_padded_so_the_ids_line_up() {
        // Alignment is a fact about the list, not about one row: every owner is
        // drawn to the width of the widest, so the ids start in the same column and
        // the eye can run down them. The dash is padded like any other owner.
        let cache = Cache::new();
        let workspaces = cache.listed(
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
            offered(&workspaces, cache.path())
                .offers
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
        let cache = Cache::new();
        let workspaces = cache.listed(
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
            offered(&workspaces, cache.path())
                .offers
                .iter()
                .map(|offer| offer.workspace_id.clone())
                .collect::<Vec<_>>(),
            ["zebra", "alpha"]
        );
    }

    #[test]
    fn an_empty_listing_offers_nothing_and_is_not_a_picker() {
        let cache = Cache::new();
        let none = cache.listed("[]");

        assert!(offered(&none, cache.path()).offers.is_empty());
        // And no terminal is opened to say so: nothing to pick from is answered
        // before anything is drawn, whichever arity asked.
        assert_eq!(pick(&none, Arity::One, cache.path()), Pick::NoWorkspaces);
        assert_eq!(
            pick(&none, Arity::Several, cache.path()),
            Pick::NoWorkspaces
        );
    }

    #[test]
    fn a_row_that_names_no_workspace_is_no_choice() {
        // Python's `ws_map.get(selected)`: a label the map has not got answers
        // `None`, and `None` is the help and exit 1.
        let cache = Cache::new();
        let offers = offered(
            &cache.listed(&one("mine", r#"{"localFolder": "/p"}"#)),
            cache.path(),
        )
        .offers;

        assert_eq!(chosen(&offers, Vec::new()), Pick::Quit);
        assert_eq!(
            chosen(&offers, vec!["something else".to_owned()]),
            Pick::Quit
        );
    }

    #[test]
    fn the_whole_table_lines_up_under_its_own_headings() {
        // Every other test here asks about one row shape. This one asks the
        // question the shapes were split up to answer: put all of them in one
        // listing and does it read as a table?
        //
        // Four shapes at once -- a pair that collided and says its id, a row with a
        // long branch that no other row has, a row on a short branch, and a
        // workspace dl did not clone -- because the widths are a fact about the set
        // and a shape that is fine alone can still be the one that does not line up
        // beside the others. Asserting the drawn text is the only way to see that:
        // a per-column assertion agrees with itself by construction.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-legacy", "main");
        cache.clone_at(
            "blooop",
            "wayfinder",
            "wayfinder-wf-467-a2b3",
            "wayfinder/devlaunch-467",
        );
        cache.clone_at("myfork", "bencher", "bencher-fix-x9q2", "fix/thing");
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t", "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"}, "lastUsed": "x", "provider": {"name": "docker"}, "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-main-legacy", "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-legacy"}, "lastUsed": "x", "provider": {"name": "docker"}, "ide": {"name": "none"}, "context": "default"},
                {"id": "wayfinder-wf-467-a2b3", "source": {"localFolder": "{CACHE}/repos/blooop/wayfinder/wayfinder-wf-467-a2b3"}, "lastUsed": "x", "provider": {"name": "docker"}, "ide": {"name": "none"}, "context": "default"},
                {"id": "bencher-fix-x9q2", "source": {"localFolder": "{CACHE}/repos/myfork/bencher/bencher-fix-x9q2"}, "lastUsed": "x", "provider": {"name": "docker"}, "ide": {"name": "none"}, "context": "default"},
                {"id": "someones-project", "source": {"localFolder": "/home/dev/someones-project"}, "lastUsed": "x", "provider": {"name": "docker"}, "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );
        let offering = offered(&workspaces, cache.path());

        let mut drawn = vec![offering.heading.clone()];
        drawn.extend(offering.offers.iter().map(|offer| offer.label.clone()));
        assert_eq!(
            drawn,
            [
                "OWNER  | REPO             | BRANCH                  | WORKSPACE",
                "blooop | devlaunch        | main                    | devlaunch-main-3j1t",
                "blooop | devlaunch        | main                    | devlaunch-main-legacy",
                "blooop | wayfinder        | wayfinder/devlaunch-467",
                "myfork | bencher          | fix/thing",
                "-      | someones-project",
            ]
        );
        // And the claim that assertion is standing for, said as a rule rather than
        // as one drawing: a cell that has anything after it starts where the same
        // cell starts on the heading. The rows that end early are the ones the rule
        // says nothing about, which is what `trimmed` is for.
        let column_starts = |line: &str| {
            let mut at = 0;
            line.split(" | ")
                .map(|cell| {
                    let start = at;
                    at += cell.chars().count() + " | ".chars().count();
                    start
                })
                .collect::<Vec<_>>()
        };
        let headings = column_starts(&offering.heading);
        for offer in &offering.offers {
            let row = column_starts(&offer.label);
            assert_eq!(
                row,
                headings[..row.len()],
                "a row's columns start somewhere its headings do not: {:?} under {:?}",
                offer.label,
                offering.heading,
            );
        }
    }

    #[test]
    fn the_heading_names_a_column_exactly_when_some_row_has_something_in_it() {
        // The picker is a table, so it says what its columns are -- and it says it
        // in skim's header rather than as a row, which is what keeps it from being
        // filtered away, marked with TAB, or picked.
        //
        // Three shapes, one rule: a heading appears when a cell under it does. A
        // listing of nothing dl cloned has no branch to head and no id column, so
        // heading either would put a word over empty air.
        let cache = Cache::new();
        let foreign = cache.listed(&one("myproject", r#"{"localFolder": "/home/dev/mine"}"#));
        assert_eq!(offered(&foreign, cache.path()).heading, "OWNER | REPO");

        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        let cloned = cache.listed(&one(
            "devlaunch-main-3j1t",
            r#"{"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"}"#,
        ));
        assert_eq!(
            offered(&cloned, cache.path()).heading,
            "OWNER  | REPO      | BRANCH"
        );

        // And the fourth column, which exists only where a collision drew it.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-legacy", "main");
        let collided = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-main-legacy",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-legacy"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );
        assert_eq!(
            offered(&collided, cache.path()).heading,
            "OWNER  | REPO      | BRANCH | WORKSPACE"
        );
    }

    #[test]
    fn the_heading_is_measured_as_a_cell_so_it_cannot_overhang_its_own_column() {
        // A column whose entries are all narrower than the word over them would draw
        // the word into the next column, and every heading after it would sit one
        // place right of the cells it names. The heading is measured with the cells
        // for exactly that reason -- so the owners here are padded out to `OWNER`
        // rather than the heading being cut down to `-`.
        //
        // `dl --ls` does the opposite for its `WORKSPACE` column, and on purpose:
        // that arithmetic is Python's, quirk and all, and the harness compares it
        // byte for byte. This table is nobody's byte-for-byte.
        let cache = Cache::new();
        let workspaces = cache.listed(
            r#"[
                {"id": "a", "source": {"localFolder": "/a"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"},
                {"id": "b", "source": {"localFolder": "/b"}, "lastUsed": "x",
                 "provider": {"name": "docker"}, "ide": {"name": "none"},
                 "context": "default"}
            ]"#,
        );

        let offering = offered(&workspaces, cache.path());

        assert_eq!(offering.heading, "OWNER | REPO");
        assert_eq!(
            offering
                .offers
                .iter()
                .map(|offer| offer.label.clone())
                .collect::<Vec<_>>(),
            ["-     | a", "-     | b"]
        );
    }

    #[test]
    fn a_row_never_ends_in_the_padding_its_last_cell_did_not_need() {
        // Every cell is padded to its column, and then the line's end is trimmed.
        // The trailing run is invisible on a terminal and merely wrong everywhere
        // else: `unpadded` is quoted inside a sentence by `render::picked`, and a
        // `dl rm` receipt reading `Picked blooop | devlaunch | main    -> ...` is
        // the shape of the bug.
        //
        // The rows here make the case that has something to trim: one long branch
        // sets the branch column's width, and the short branch beside it ends the
        // line rather than paying it.
        let cache = Cache::new();
        cache.clone_at("blooop", "devlaunch", "devlaunch-main-3j1t", "main");
        cache.clone_at(
            "blooop",
            "devlaunch",
            "devlaunch-a-very-long-branch-9x2k",
            "a/very/long/branch/name",
        );
        let workspaces = cache.listed(
            r#"[
                {"id": "devlaunch-main-3j1t",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-main-3j1t"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"},
                {"id": "devlaunch-a-very-long-branch-9x2k",
                 "source": {"localFolder": "{CACHE}/repos/blooop/devlaunch/devlaunch-a-very-long-branch-9x2k"},
                 "lastUsed": "x", "provider": {"name": "docker"},
                 "ide": {"name": "none"}, "context": "default"}
            ]"#,
        );

        let offering = offered(&workspaces, cache.path());

        for offer in &offering.offers {
            assert_eq!(
                offer.label,
                offer.label.trim_end(),
                "a label ends in padding: {:?}",
                offer.label
            );
            assert_eq!(
                offer.unpadded,
                offer.unpadded.trim_end(),
                "an unpadded row ends in padding: {:?}",
                offer.unpadded
            );
        }
        assert_eq!(
            offering.heading,
            offering.heading.trim_end(),
            "the heading ends in padding: {:?}",
            offering.heading
        );
        assert_eq!(
            offering.offers[0].label, "blooop | devlaunch | main",
            "the short branch ends its line rather than paying the column's width"
        );
    }
}
