//! How much disk a directory would give back — the one number, measured once.
//!
//! devlaunch reports two disk figures and they are the same question asked of
//! different directories: `dl --ls --size` asks it of the clone behind a live
//! workspace, and the orphan report asks it of a clone no workspace references
//! any more. Both want *what deleting this would free*, so both call
//! [`exclusive_usage`] and neither grows its own walk.
//!
//! **Exclusive, not apparent, and the difference is the whole point.** A repo's
//! bare cache holds one copy of its git objects and every workspace clone
//! hardlinks out of it (devlaunch#154), which is where most of the saving on a
//! repo's second workspace comes from. Walking one clone on its own and adding up
//! the blocks its files occupy — what `du` reports when pointed at one directory
//! — counts that shared pool in full, so a repo's workspaces each read as most of
//! the disk they share: the design's own saving, reported as if it had never
//! happened.
//!
//! **The measurement this module is documented from.** One real clone of a ROS
//! repo, made by `git clone` from the bare in devlaunch's own cache, on Ubuntu
//! 24.04 / ext4 with a warm page cache:
//!
//! | | |
//! |---|---|
//! | `du -s --block-size=1` on the clone alone | 353,230,848 B |
//! | `exclusive_usage` on the clone | 68,050,944 B |
//! | `exclusive_usage` on the bare it clones from | 651,264 B |
//! | `du -sc --block-size=1` over both together | 353,882,112 B |
//!
//! `du` bills that one workspace **5.2x** what deleting it would free. The
//! difference is one 270,823,424-byte pack file with two links, one in the clone
//! and one in the bare: removing either end frees nothing, and the bytes go to
//! whichever is last. Note the last two rows against each other — the exclusive
//! figures sum to 68,702,208 B while the disk holds 353,882,112 B, which is the
//! first of the two consequences below and not an error.
//!
//! So a file's bytes are billed to a tree only when **every one of its links lies
//! inside that tree**. Two consequences worth stating plainly, because they are
//! properties rather than bugs:
//!
//! - Exclusive bytes do not sum to total disk. A pool shared by three workspaces
//!   is billed to none of them, because deleting any one of them frees none of
//!   it. It becomes the last holder's the moment it is the last holder, which is
//!   exactly when deleting it would free the bytes.
//! - The figure is a fact about the tree *and its neighbours*, so it changes when
//!   a sibling is deleted without the tree itself changing. That is the truth
//!   about shared storage; a number that stayed still would be the lie.
//!
//! Sizes are bytes actually allocated (`st_blocks`), not lengths: a sparse file
//! and a tree of many small files both cost what the filesystem gave them, which
//! is what a reclamation figure has to be about.
//!
//! Ported from `devlaunch/disk_usage.py`; see docs/rust-rewrite-plan.md (M5).
//! Python's `_unhandled_usage` shim — a hand-rolled `assert_never` for the two
//! arms — has no analogue: `match` on [`DiskUsage`] is exhaustive by the
//! compiler, which is the guarantee that shim was reaching for.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// `st_blocks` is counted in 512-byte units by POSIX, whatever the filesystem's
/// own block size is.
const BLOCK: u64 = 512;

/// The binary units a size reads in, smallest first.
const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];

/// The doors a walk could not open: at least one, or the walk was not a floor.
///
/// Built from the positive space rather than checked: a floor whose door list is
/// empty would be a `≥` nobody can act on and a `"unreadable": 0` in the JSON,
/// and it is the shape a bug in the walk would produce. Here it cannot be
/// constructed.
///
/// The paths are carried rather than counted so a caller can say *which* door,
/// which is the difference between a report someone can act on and a caveat.
///
/// `pub` only because the [`DiskUsage::PartlyUnreadable`] variant carries it
/// and variant fields inherit the enum's visibility; the binary matches on
/// `DiskUsage` without naming this type.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedDoors {
    first: PathBuf,
    rest: Vec<PathBuf>,
}

impl ClosedDoors {
    /// The doors in `paths`, or `None` if there were none — which is what makes
    /// a walk [`DiskUsage::Measured`] rather than a floor.
    pub(crate) fn of(paths: Vec<PathBuf>) -> Option<Self> {
        let mut paths = paths.into_iter();
        let first = paths.next()?;
        Some(ClosedDoors {
            first,
            rest: paths.collect(),
        })
    }

    /// One door.
    pub(crate) fn one(path: impl Into<PathBuf>) -> Self {
        ClosedDoors {
            first: path.into(),
            rest: Vec::new(),
        }
    }

    /// Every door, in the order it was met.
    ///
    /// Only this module's tests walk them; the binary names the first and counts
    /// the rest.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.first.as_path()).chain(self.rest.iter().map(PathBuf::as_path))
    }

    /// How many doors — at least one.
    pub(crate) fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Every door of `other`, after this one's.
    fn extend(&mut self, other: ClosedDoors) {
        self.rest.push(other.first);
        self.rest.extend(other.rest);
    }
}

/// What a walk of one tree found.
///
/// Two arms and no third: "the directory is not there" is not a failure to
/// measure, it is a measurement of nothing — `Measured { exclusive_bytes: 0 }`,
/// the same answer the listing gives for a clone that has already been removed by
/// hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskUsage {
    /// A complete walk: what removing the tree would free.
    Measured { exclusive_bytes: u64 },
    /// A walk that hit a door it could not open, and what it got to before that.
    ///
    /// A clone is written into by a container running as another user, so a
    /// directory this process cannot read is the ordinary case here rather than
    /// the exotic one. The bytes behind that door are unknown, which makes
    /// `at_least_bytes` a floor and not a total — and reporting a floor as a
    /// total is how a cleanup tool tells someone a workspace is small when it is
    /// not.
    PartlyUnreadable {
        at_least_bytes: u64,
        unreadable: ClosedDoors,
    },
}

impl DiskUsage {
    /// A complete measurement of `bytes`.
    ///
    /// `pub` for the binary's sake — its rendering tests build the cell the table
    /// draws from, and there is no reachable measurement to borrow inside a unit
    /// test. Binary surface, not part of the frozen `wf` API.
    pub fn measured(bytes: u64) -> Self {
        DiskUsage::Measured {
            exclusive_bytes: bytes,
        }
    }

    /// `bytes` behind whichever doors `unreadable` holds — a total when it holds
    /// none, which is the one place the two arms are chosen between.
    fn of(bytes: u64, unreadable: Vec<PathBuf>) -> Self {
        match ClosedDoors::of(unreadable) {
            None => DiskUsage::measured(bytes),
            Some(doors) => DiskUsage::PartlyUnreadable {
                at_least_bytes: bytes,
                unreadable: doors,
            },
        }
    }

    /// The bytes this usage accounts for, whichever arm it is.
    ///
    /// For the callers that have to put usages in an order or add them up —
    /// "which of these is worth reclaiming first" — where a floor and a total are
    /// both usable because the question is comparative.
    ///
    /// It is deliberately not how a usage is *reported*: printing this number
    /// stripped of its arm is exactly how a floor gets read as a total, which is
    /// what [`describe_usage`] and [`usage_as_json`] exist to prevent.
    pub(crate) fn known_bytes(&self) -> u64 {
        match self {
            DiskUsage::Measured { exclusive_bytes } => *exclusive_bytes,
            DiskUsage::PartlyUnreadable { at_least_bytes, .. } => *at_least_bytes,
        }
    }

    /// The doors this usage could not open — none, for a complete walk.
    fn doors(self) -> Option<ClosedDoors> {
        match self {
            DiskUsage::Measured { .. } => None,
            DiskUsage::PartlyUnreadable { unreadable, .. } => Some(unreadable),
        }
    }
}

/// What removing `tree` would free, or how far the walk got before a refusal.
///
/// One `lstat` per entry and no subprocess, so this is safe to call from anywhere
/// that is not a hot path — but it is O(files) with no ceiling, which is why
/// `--ls` asks for it only when told to.
///
/// Symlinks are weighed as themselves and never followed: a link into someone's
/// home directory is a few bytes of this tree, not a claim on what it points at.
/// Directories are always this tree's — Linux does not hardlink them — so their
/// own blocks count without consulting the link count, which for a directory says
/// how many children it has rather than whether it is shared.
///
/// A tree being used changes under the walk, so anything that stops existing
/// between being named and being weighed is worth nothing rather than unknown: it
/// frees nothing now, which is a measurement. Only a door that will not open
/// makes the answer a floor.
pub(crate) fn exclusive_usage(tree: &Path) -> DiskUsage {
    let root = match std::fs::symlink_metadata(tree) {
        Ok(root) => root,
        // Nothing there is nothing to free, and that is an answer.
        Err(error) if vanished(&error) => return DiskUsage::measured(0),
        Err(_) => {
            return DiskUsage::PartlyUnreadable {
                at_least_bytes: 0,
                unreadable: ClosedDoors::one(tree),
            };
        }
    };

    if !root.is_dir() {
        return DiskUsage::measured(allocated(&root));
    }

    let mut total = allocated(&root);
    let mut unreadable: Vec<PathBuf> = Vec::new();
    // (device, inode) -> the links found inside this tree, its bytes, and how
    // many links it has in all.
    let mut files: std::collections::HashMap<(u64, u64), Shared> = std::collections::HashMap::new();
    let mut pending = vec![tree.to_path_buf()];

    while let Some(directory) = pending.pop() {
        let Some(found) = read_directory(&directory, &mut unreadable) else {
            continue;
        };
        for entry in found {
            let path = entry.path();
            let Some(stat) = weigh(entry.metadata(), &path, &mut unreadable) else {
                continue;
            };
            if stat.is_dir() {
                total = total.saturating_add(allocated(&stat));
                pending.push(path);
                continue;
            }
            files
                .entry((stat.dev(), stat.ino()))
                .and_modify(|shared| shared.links_here = shared.links_here.saturating_add(1))
                .or_insert(Shared {
                    links_here: 1,
                    bytes: allocated(&stat),
                    links: stat.nlink(),
                });
        }
    }

    // A file's bytes are this tree's only when every one of its links is inside
    // it: that is when removing the tree is what frees them.
    for shared in files.values() {
        if shared.links_here >= shared.links {
            total = total.saturating_add(shared.bytes);
        }
    }
    // Sorted so a report names the doors in a stable order rather than in
    // whatever order the walk met them.
    unreadable.sort();
    DiskUsage::of(total, unreadable)
}

/// One inode seen during a walk: how many of its links are inside the tree, what
/// it costs, and how many links it has in all.
#[derive(Debug)]
struct Shared {
    links_here: u64,
    bytes: u64,
    links: u64,
}

/// The entries of `directory`, or `None` if there are none to be had.
///
/// Nothing is a floor unless a door refused to open: a directory that vanished
/// between being named and being opened is the same race as an entry that
/// vanished, one level up, and is answered the same way.
fn read_directory(
    directory: &Path,
    unreadable: &mut Vec<PathBuf>,
) -> Option<Vec<std::fs::DirEntry>> {
    let listing = match std::fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(error) if vanished(&error) => return None,
        Err(_) => {
            unreadable.push(directory.to_path_buf());
            return None;
        }
    };
    let mut found = Vec::new();
    for entry in listing {
        match entry {
            Ok(entry) => found.push(entry),
            Err(error) => {
                // Reading the directory itself failed part-way through, so what
                // is behind the rest of it is unknown — including the entries
                // already collected, whose siblings are missing from the total.
                if !vanished(&error) {
                    unreadable.push(directory.to_path_buf());
                }
                return None;
            }
        }
    }
    Some(found)
}

/// The stat of something the walk named, or `None` with the reason recorded.
///
/// The whole race/refusal rule in one place: a name that came back from a
/// directory and was gone before it could be weighed frees nothing *now*, which
/// is a measurement — a live cache does that on its own, git repacks, a container
/// writes — where a door that will not open leaves bytes nobody can count, which
/// is a floor. Calling a race a closed door would turn an ordinary listing into a
/// floor.
fn weigh(
    stat: io::Result<std::fs::Metadata>,
    path: &Path,
    unreadable: &mut Vec<PathBuf>,
) -> Option<std::fs::Metadata> {
    match stat {
        Ok(stat) => Some(stat),
        Err(error) if vanished(&error) => None,
        Err(_) => {
            unreadable.push(path.to_path_buf());
            None
        }
    }
}

/// Whether an error says the thing is simply not there.
fn vanished(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

/// Bytes the filesystem actually gave this inode.
fn allocated(stat: &std::fs::Metadata) -> u64 {
    stat.blocks().saturating_mul(BLOCK)
}

/// What removing all of `usages`' trees together would free.
///
/// A sum with one floor in it is a floor, and this returns the arm that says so.
/// That is the whole reason a total lives here rather than in the caller: adding
/// [`DiskUsage::known_bytes`] up gives an integer that has lost which kind of
/// answer it is, and an integer printed as a size is a floor read as a total —
/// the one mistake every other function in this module is shaped to prevent.
///
/// Which bytes these are, and why they do not add up to the disk a cache holds,
/// is the module docs' business and is not restated here.
pub(crate) fn total_usage(usages: impl IntoIterator<Item = DiskUsage>) -> DiskUsage {
    let mut known: u64 = 0;
    let mut doors: Option<ClosedDoors> = None;
    for usage in usages {
        known = known.saturating_add(usage.known_bytes());
        if let Some(more) = usage.doors() {
            match doors.as_mut() {
                None => doors = Some(more),
                Some(carried) => carried.extend(more),
            }
        }
    }
    match doors {
        None => DiskUsage::measured(known),
        Some(unreadable) => DiskUsage::PartlyUnreadable {
            at_least_bytes: known,
            unreadable,
        },
    }
}

/// How a usage reads to a person: a size, or a floor marked as one.
///
/// The one rendering core keeps, because the `≥` is not a word anyone will
/// translate — it is what stops a floor from being read as a total, and a caller
/// that formatted the bytes itself would have to remember to add it.
pub fn describe_usage(usage: &DiskUsage) -> String {
    match usage {
        DiskUsage::Measured { exclusive_bytes } => human(*exclusive_bytes),
        DiskUsage::PartlyUnreadable { at_least_bytes, .. } => {
            format!("≥{}", human(*at_least_bytes))
        }
    }
}

/// `count` bytes in the largest binary unit that leaves it above one.
fn human(count: u64) -> String {
    if count < 1024 {
        return format!("{count} B");
    }
    let mut size = count as f64 / 1024.0;
    let mut unit = UNITS[0];
    for larger in &UNITS[1..] {
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
        unit = larger;
    }
    format!("{size:.1} {unit}")
}

/// How a usage reads to a tool: one key, and the key says which kind it is.
///
/// Deliberately not a bare integer with a flag beside it. A caller that reads
/// `exclusiveBytes` has a total; a caller that reads `atLeastBytes` has a floor
/// and cannot have got there by ignoring a field.
///
/// This is the Grade-A wire shape `--ls --json` splices in, so the key spellings
/// are the contract.
pub(crate) fn usage_as_json(usage: &DiskUsage) -> serde_json::Value {
    serde_json::to_value(UsageWire::of(usage)).unwrap_or_else(|_| serde_json::json!({}))
}

/// The wire re-encoding: the arm chooses the key, and the doors become a count.
///
/// Flattened here rather than by deriving on [`DiskUsage`], because the wire
/// reports how many doors there were and not which — the paths are for a person,
/// and the count is what a tool can act on.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum UsageWire {
    Measured {
        #[serde(rename = "exclusiveBytes")]
        exclusive_bytes: u64,
    },
    Floor {
        #[serde(rename = "atLeastBytes")]
        at_least_bytes: u64,
        unreadable: usize,
    },
}

impl UsageWire {
    fn of(usage: &DiskUsage) -> Self {
        match usage {
            DiskUsage::Measured { exclusive_bytes } => UsageWire::Measured {
                exclusive_bytes: *exclusive_bytes,
            },
            DiskUsage::PartlyUnreadable {
                at_least_bytes,
                unreadable,
            } => UsageWire::Floor {
                at_least_bytes: *at_least_bytes,
                unreadable: unreadable.len(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    //! What `test/unit/test_disk_usage.py` pins, on this side of the port.
    //!
    //! The number under test is *exclusive* bytes: what deleting this directory
    //! would free, counting a file only when every one of its links lies inside
    //! the tree being measured. Why that rather than an apparent size, and the
    //! measurement that settled it, are in the module docs and not repeated here
    //! — these tests are the pin on the choice, not a second copy of the argument
    //! for it. Swapping in an apparent size fails the hardlink cases and nothing
    //! else.
    //!
    //! Python staged its two mid-walk races by patching `os.scandir`. There is no
    //! module-global to patch here, so the rule those tests were about — a name
    //! that vanished is worth nothing, a door that refused is a floor — is pinned
    //! on [`weigh`] and [`read_directory`], the two functions that decide it,
    //! plus the one race that can be staged for real: a tree that is not there.

    use super::*;
    use std::fs;
    use std::path::Path;

    const MIB: u64 = 1024 * 1024;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    /// Write `mib` MiB of real bytes at `path`, making its parents.
    fn payload(path: &Path, mib: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the parents");
        }
        let size = usize::try_from(mib * MIB).expect("a size this machine can hold");
        fs::write(path, vec![0u8; size]).expect("the payload");
    }

    /// The byte count of a complete measurement, failing if it was not one.
    fn measured_bytes(usage: &DiskUsage) -> u64 {
        match usage {
            DiskUsage::Measured { exclusive_bytes } => *exclusive_bytes,
            other => panic!("expected a complete measurement, got {other:?}"),
        }
    }

    fn doors_of(usage: &DiskUsage) -> Vec<&Path> {
        match usage {
            DiskUsage::PartlyUnreadable { unreadable, .. } => unreadable.paths().collect(),
            other => panic!("expected a floor, got {other:?}"),
        }
    }

    /// Root is refused by nothing, so a closed door would open for it.
    fn not_as_root() -> bool {
        // SAFETY: `geteuid` reads one integer out of the process and touches
        // nothing; it cannot fail and has no preconditions.
        unsafe { libc::geteuid() != 0 }
    }

    // --- what a tree holds ------------------------------------------------

    #[test]
    fn a_tree_of_known_content_reports_what_it_holds() {
        let dir = temp_dir();
        let tree = dir.path().join("tree");
        for name in ["a.bin", "b.bin", "nested/c.bin", "nested/deep/d.bin"] {
            payload(&tree.join(name), 1);
        }

        let got = measured_bytes(&exclusive_usage(&tree));

        // Four MiB of payload, plus a handful of directory blocks — and not the
        // eight MiB an implementation that counted each file twice reports.
        assert!((4 * MIB..5 * MIB).contains(&got), "{got}");
    }

    #[test]
    fn a_directory_that_is_not_there_holds_nothing() {
        let dir = temp_dir();

        assert_eq!(
            exclusive_usage(&dir.path().join("never-existed")),
            DiskUsage::measured(0)
        );
    }

    #[test]
    fn an_empty_directory_costs_only_itself() {
        let dir = temp_dir();
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).expect("the directory");

        assert!(measured_bytes(&exclusive_usage(&empty)) < MIB);
    }

    #[test]
    fn a_file_handed_in_instead_of_a_directory_is_worth_itself() {
        // The caller that classifies clone directories walks whatever it found;
        // a stray file where a clone was expected is a measurement, not a crash.
        let dir = temp_dir();
        let stray = dir.path().join("stray.bin");
        payload(&stray, 1);

        let got = measured_bytes(&exclusive_usage(&stray));

        assert!((MIB..2 * MIB).contains(&got), "{got}");
    }

    #[test]
    fn a_symlink_is_worth_its_own_size_not_its_targets() {
        let dir = temp_dir();
        let big = dir.path().join("outside").join("big.bin");
        payload(&big, 4);
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).expect("the tree");
        std::os::unix::fs::symlink(&big, tree.join("link")).expect("the link");

        assert!(measured_bytes(&exclusive_usage(&tree)) < MIB);
    }

    // --- sharing is not billed twice --------------------------------------

    #[test]
    fn a_file_hardlinked_from_outside_is_not_billed_to_the_tree() {
        // The shape devlaunch actually creates: a bare cache holding the packs,
        // and a workspace clone whose pack file is the same inode.
        let dir = temp_dir();
        let shared = dir.path().join("bare").join("shared.pack");
        payload(&shared, 4);
        let clone = dir.path().join("clone");
        payload(&clone.join("own.bin"), 1);
        fs::hard_link(&shared, clone.join("shared.pack")).expect("the link");

        let got = measured_bytes(&exclusive_usage(&clone));

        // Deleting the clone frees its own MiB and nothing else: the bare keeps
        // the pack. An apparent size reports 5 MiB here.
        assert!((MIB..2 * MIB).contains(&got), "{got}");
    }

    #[test]
    fn the_holder_of_the_last_link_is_billed_for_it() {
        // Same pool, measured from the side that is about to become the only
        // holder — so the bytes are not simply lost from every report.
        let dir = temp_dir();
        let bare = dir.path().join("bare");
        let shared = bare.join("shared.pack");
        payload(&shared, 4);
        let clone = dir.path().join("clone");
        fs::create_dir(&clone).expect("the clone");
        let borrowed = clone.join("shared.pack");
        fs::hard_link(&shared, &borrowed).expect("the link");
        fs::remove_file(&borrowed).expect("giving the link back");

        let got = measured_bytes(&exclusive_usage(&bare));

        assert!((4 * MIB..5 * MIB).contains(&got), "{got}");
    }

    #[test]
    fn a_file_hardlinked_twice_within_the_tree_is_counted_once() {
        let dir = temp_dir();
        let tree = dir.path().join("tree");
        payload(&tree.join("a.bin"), 2);
        fs::hard_link(tree.join("a.bin"), tree.join("b.bin")).expect("the link");

        // Both links are inside, so the bytes are the tree's — once.
        let got = measured_bytes(&exclusive_usage(&tree));

        assert!((2 * MIB..3 * MIB).contains(&got), "{got}");
    }

    #[test]
    fn a_pool_shared_by_two_measured_trees_is_billed_to_neither() {
        // The property the module docs call the first consequence: exclusive
        // bytes do not sum to total disk.
        let dir = temp_dir();
        let bare = dir.path().join("bare");
        payload(&bare.join("shared.pack"), 4);
        let clone = dir.path().join("clone");
        fs::create_dir(&clone).expect("the clone");
        fs::hard_link(bare.join("shared.pack"), clone.join("shared.pack")).expect("the link");

        let total = total_usage([exclusive_usage(&bare), exclusive_usage(&clone)]);

        assert!(total.known_bytes() < MIB, "{total:?}");
    }

    // --- what could not be read -------------------------------------------

    #[test]
    fn an_unreadable_directory_makes_the_answer_a_floor() {
        if !not_as_root() {
            return;
        }
        let dir = temp_dir();
        let tree = dir.path().join("tree");
        payload(&tree.join("readable.bin"), 2);
        let locked = tree.join("locked");
        payload(&locked.join("hidden.bin"), 4);
        shut(&locked, 0o000);

        let usage = exclusive_usage(&tree);
        shut(&locked, 0o700);

        assert_eq!(doors_of(&usage), [locked.as_path()]);
        // What was readable is still reported, as a floor: the hidden 4 MiB are
        // missing from it, which is exactly why it is not called a total.
        assert!(
            (2 * MIB..3 * MIB).contains(&usage.known_bytes()),
            "{usage:?}"
        );
    }

    #[test]
    fn a_tree_that_cannot_be_opened_at_all_reports_no_total_either() {
        if !not_as_root() {
            return;
        }
        let dir = temp_dir();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).expect("the tree");
        shut(&tree, 0o000);

        let usage = exclusive_usage(&tree);
        shut(&tree, 0o700);

        assert_eq!(doors_of(&usage), [tree.as_path()]);
    }

    #[test]
    fn a_tree_behind_a_closed_parent_reports_no_total_either() {
        // Not even the first `lstat` gets through — that needs the parent to be
        // traversable — so there is nothing to report but the closed door.
        if !not_as_root() {
            return;
        }
        let dir = temp_dir();
        let outer = dir.path().join("outer");
        let tree = outer.join("tree");
        payload(&tree.join("file.bin"), 1);
        shut(&outer, 0o000);

        let usage = exclusive_usage(&tree);
        shut(&outer, 0o700);

        assert_eq!(
            usage,
            DiskUsage::PartlyUnreadable {
                at_least_bytes: 0,
                unreadable: ClosedDoors::one(&tree),
            }
        );
    }

    #[test]
    fn entries_that_can_be_named_but_not_stat_ed_are_named() {
        // Readable but not traversable: the names come back from the directory
        // itself, and every `stat` on them is refused. The bytes behind them are
        // unknown, so they are listed rather than quietly counted as nothing.
        if !not_as_root() {
            return;
        }
        let dir = temp_dir();
        let tree = dir.path().join("tree");
        let listed = tree.join("listed");
        payload(&listed.join("hidden.bin"), 2);
        shut(&listed, 0o444);

        let usage = exclusive_usage(&tree);
        shut(&listed, 0o700);

        assert!(
            doors_of(&usage).contains(&listed.join("hidden.bin").as_path()),
            "{usage:?}"
        );
    }

    #[test]
    fn the_doors_of_one_walk_are_named_in_a_stable_order() {
        if !not_as_root() {
            return;
        }
        let dir = temp_dir();
        let tree = dir.path().join("tree");
        for name in ["b-locked", "a-locked", "c-locked"] {
            let locked = tree.join(name);
            payload(&locked.join("hidden.bin"), 1);
            shut(&locked, 0o000);
        }

        let usage = exclusive_usage(&tree);
        for name in ["b-locked", "a-locked", "c-locked"] {
            shut(&tree.join(name), 0o700);
        }

        assert_eq!(
            doors_of(&usage),
            [
                tree.join("a-locked").as_path(),
                tree.join("b-locked").as_path(),
                tree.join("c-locked").as_path(),
            ]
        );
    }

    fn shut(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("the mode");
    }

    // --- what vanishes mid-walk -------------------------------------------

    #[test]
    fn a_name_that_vanished_before_it_was_weighed_is_worth_nothing() {
        // A live cache changes under the walk — git repacks, a container writes,
        // a sibling command deletes — and a walk that reported a floor every time
        // would make `≥` the usual answer for no reason.
        let mut unreadable = Vec::new();

        let stat = weigh(
            Err(io::Error::from(io::ErrorKind::NotFound)),
            Path::new("/cache/goes.bin"),
            &mut unreadable,
        );

        assert!(stat.is_none());
        assert_eq!(
            unreadable,
            [] as [PathBuf; 0],
            "a race is not a closed door"
        );
    }

    #[test]
    fn a_name_that_could_not_be_weighed_is_a_closed_door() {
        let mut unreadable = Vec::new();

        let stat = weigh(
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            Path::new("/cache/hidden.bin"),
            &mut unreadable,
        );

        assert!(stat.is_none());
        assert_eq!(unreadable, [PathBuf::from("/cache/hidden.bin")]);
    }

    #[test]
    fn a_directory_that_vanished_before_it_was_opened_is_worth_nothing() {
        // The same race one level up, answered the same way: a total, not a
        // floor.
        let dir = temp_dir();
        let mut unreadable = Vec::new();

        let found = read_directory(&dir.path().join("goes"), &mut unreadable);

        assert!(found.is_none());
        assert_eq!(unreadable, [] as [PathBuf; 0]);
    }

    // --- comparing usages whatever arm they are ---------------------------

    #[test]
    fn a_measurement_offers_its_total_and_a_floor_offers_the_floor() {
        // The accessor a caller that ranks or sums usages needs, so it does not
        // reach into the arms and hand-roll an `else`. `dl --prune` is the
        // caller: "which of these is worth reclaiming first" is answerable from a
        // floor as well as from a total, and it is the only question that is.
        let floor = DiskUsage::PartlyUnreadable {
            at_least_bytes: 1536,
            unreadable: ClosedDoors::one("/x"),
        };

        assert_eq!(DiskUsage::measured(1536).known_bytes(), 1536);
        assert_eq!(floor.known_bytes(), 1536);
        // The same number from either arm, which is exactly why the renderings
        // do not go through here: only they keep the floor visible as one.
        assert_ne!(
            describe_usage(&floor),
            describe_usage(&DiskUsage::measured(1536))
        );
    }

    // --- adding usages up -------------------------------------------------

    #[test]
    fn complete_walks_add_up_to_a_complete_total() {
        assert_eq!(
            total_usage([DiskUsage::measured(1024), DiskUsage::measured(512)]),
            DiskUsage::measured(1536)
        );
    }

    #[test]
    fn nothing_at_all_adds_up_to_nothing_measured() {
        assert_eq!(total_usage([]), DiskUsage::measured(0));
    }

    #[test]
    fn one_floor_among_them_makes_the_total_a_floor() {
        // The caller is `dl --prune`, which prints "removing N directories — X".
        // Adding known bytes up gives an integer that has forgotten whether any
        // part of it was a floor.
        let total = total_usage([
            DiskUsage::measured(1024),
            DiskUsage::PartlyUnreadable {
                at_least_bytes: 512,
                unreadable: ClosedDoors::one("/x"),
            },
        ]);

        assert_eq!(
            total,
            DiskUsage::PartlyUnreadable {
                at_least_bytes: 1536,
                unreadable: ClosedDoors::one("/x"),
            }
        );
        assert_eq!(describe_usage(&total), "≥1.5 KiB");
    }

    #[test]
    fn every_closed_door_is_carried_into_the_total() {
        // The paths are what makes the caveat actionable rather than a warning: a
        // person told which directories were not counted can go and look.
        let total = total_usage([
            DiskUsage::PartlyUnreadable {
                at_least_bytes: 512,
                unreadable: ClosedDoors::one("/x"),
            },
            DiskUsage::PartlyUnreadable {
                at_least_bytes: 512,
                unreadable: ClosedDoors::one("/y"),
            },
        ]);

        assert_eq!(total.known_bytes(), 1024);
        assert_eq!(doors_of(&total), [Path::new("/x"), Path::new("/y")]);
    }

    // --- how a usage reads ------------------------------------------------

    #[test]
    fn a_measurement_reads_as_a_size() {
        for (bytes, expected) in [
            (0, "0 B"),
            (512, "512 B"),
            (1536, "1.5 KiB"),
            // The two ends of the measurement the module docs are written from:
            // what deleting one real clone frees, and what `du` bills it.
            (68_050_944, "64.9 MiB"),
            (353_230_848, "336.9 MiB"),
            (2_147_483_648, "2.0 GiB"),
        ] {
            assert_eq!(describe_usage(&DiskUsage::measured(bytes)), expected);
        }
    }

    #[test]
    fn a_floor_reads_as_a_floor() {
        assert_eq!(
            describe_usage(&DiskUsage::PartlyUnreadable {
                at_least_bytes: 1536,
                unreadable: ClosedDoors::one("/x"),
            }),
            "≥1.5 KiB"
        );
    }

    #[test]
    fn a_size_beyond_the_units_stays_in_the_largest_one() {
        assert!(human(u64::MAX).ends_with(" PiB"), "{}", human(u64::MAX));
    }

    #[test]
    fn a_measurement_is_json_as_the_bytes_it_is() {
        assert_eq!(
            usage_as_json(&DiskUsage::measured(1536)),
            serde_json::json!({"exclusiveBytes": 1536})
        );
    }

    #[test]
    fn a_floor_is_json_that_cannot_be_mistaken_for_a_total() {
        let mut doors = ClosedDoors::one("/x");
        doors.extend(ClosedDoors::one("/y"));

        assert_eq!(
            usage_as_json(&DiskUsage::PartlyUnreadable {
                at_least_bytes: 1536,
                unreadable: doors,
            }),
            serde_json::json!({"atLeastBytes": 1536, "unreadable": 2})
        );
    }

    #[test]
    fn a_floor_always_names_at_least_one_door() {
        // The invariant the `ClosedDoors` type carries: no walk can answer `≥`
        // while pointing at nothing, so no report can carry `"unreadable": 0`.
        assert_eq!(ClosedDoors::of(Vec::new()), None);
        assert_eq!(
            DiskUsage::of(1536, Vec::new()),
            DiskUsage::measured(1536),
            "no doors is a total, not a floor of the same bytes"
        );
        assert_eq!(ClosedDoors::one("/x").len(), 1);
    }
}
