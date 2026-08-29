//! Where a workspace is on this disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::clients::devpod::{Workspace, WorkspaceSource};
use crate::domain::workspace_state::NonEmpty;
use crate::flows::listing::json_as_python_writes_it;

/// Every place on this machine a source could name — possibly none.
///
/// Empty is a real answer and not a shrug: an image or container workspace opens
/// no directory on this disk, so there is nothing to compare and no clone it could
/// be holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourcePlaces {
    Placeable(Vec<String>),
    /// The source opens a folder here and devlaunch cannot say which one. Kept
    /// apart from `Placeable(vec![])` because reading them alike is how a live
    /// workspace contributed no path *and* no alarm while the command printed that
    /// it stops for exactly that.
    Unplaceable {
        payload: String,
    },
}

/// Whether `text` names a repository somewhere else rather than something on this
/// disk.
///
/// Said once, here, because both readers — `--prune`'s placement pass and
/// `--reconcile`'s orphan sweep — have to agree about it or a workspace is a
/// remote to one command and a directory to the other.
///
/// **The two mistakes are not equal, so the test is deliberately narrow.** A
/// remote read as a path is devlaunch#224: relative-looking text, resolved against
/// the current directory, so a workspace lands inside whichever repository the
/// person running `dl` happened to be standing in and is misreported there —
/// wrong, and toward refusing. A path read as a remote drops a directory out of the
/// referenced set, which is how `--prune` would come to call a live clone
/// unreferenced — wrong, and toward loss. So only the two shapes that are never
/// also written as a relative directory count: a URL scheme
/// (`[A-Za-z][A-Za-z0-9+.-]*://`), and `user@host:` where nothing before the colon
/// is a `/`. Text that is merely host-shaped (`github.com/owner/repo`) does not
/// count, because it is a perfectly good relative path, and `devpod up ./some-repo`
/// is the case that arm exists for.
///
/// `file://` matches the scheme form, and it is the one scheme that does name a
/// directory on this machine — but never usably: the callers resolve plain paths,
/// so it only ever produced `<cwd>/file:/…` garbage. Contributing nothing is
/// strictly less wrong.
pub(crate) fn names_a_remote(text: &str) -> bool {
    has_url_scheme(text) || is_scp_like(text)
}

/// `^[A-Za-z][A-Za-z0-9+.\-]*://`
fn has_url_scheme(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(|c: char| c.is_ascii_alphabetic()) else {
        return false;
    };
    let Some(at) = rest.find("://") else {
        return false;
    };
    rest[..at]
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
}

/// `^[^/@:\s]+@[^/:\s]+:` — an scp-like remote. Any `/` before the colon
/// disqualifies it, because a directory literally named that way is possible and
/// nobody's spelling.
fn is_scp_like(text: &str) -> bool {
    let Some((user, rest)) = text.split_once('@') else {
        return false;
    };
    if user.is_empty() || user.chars().any(|c| "/@:".contains(c) || c.is_whitespace()) {
        return false;
    }
    let Some((host, _)) = rest.split_once(':') else {
        return false;
    };
    !host.is_empty() && !host.chars().any(|c| "/:".contains(c) || c.is_whitespace())
}

/// Where on this machine `source` could be. Total over the arms.
///
/// A `gitRepository` counts *when it carries a path*, even though devlaunch only
/// ever hands devpod a local path, and the reason is which way the mistake runs.
/// `devpod up <path-to-a-repo>` records that arm with a path in it, and a path this
/// function does not return is a directory `--prune` will call unreferenced.
/// [`listing::is_devlaunch_clone`](crate::flows::listing::is_devlaunch_clone)
/// refuses the same arm on purpose — but refusing there means declining to delete
/// somebody else's *workspace*, which is the opposite direction, so its answer must
/// not be reused here.
pub(crate) fn source_places(source: &WorkspaceSource) -> SourcePlaces {
    match source {
        WorkspaceSource::LocalFolder(path) => SourcePlaces::Placeable(vec![path.clone()]),
        WorkspaceSource::GitRepository(url) => SourcePlaces::Placeable(if names_a_remote(url) {
            Vec::new()
        } else {
            vec![url.clone()]
        }),
        // An image or container workspace: nothing here, nothing at risk.
        WorkspaceSource::Unrecognised(_) => SourcePlaces::Placeable(Vec::new()),
        WorkspaceSource::UnreadableLocalFolder(payload) => SourcePlaces::Unplaceable {
            payload: json_as_python_writes_it(&serde_json::Value::Object(payload.clone())),
        },
    }
}

/// One live workspace whose source could not be followed, and the text that could
/// not be followed.
///
/// Not a warning above a report: a workspace whose source cannot be followed could
/// be opening *any* of the candidates, so while one exists there is no directory
/// either command can honestly call unreferenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlocatable {
    pub workspace_id: String,
    /// The source text, or devpod's own source object where there was no text to
    /// follow.
    pub detail: String,
}

/// A live workspace devpod records inside a repository's clone tree, at something
/// that is not a clone.
///
/// devlaunch#88's measured shape. On that ticket's host 36 of 39 devpod records
/// named a folder that was gone (35) or a config-only stub devpod itself wrote from
/// cache (1), while the real checkout sat beside it under the new id scheme. The
/// two records cannot be joined by workspace id — the id is exactly what changed —
/// so the join is made from the path instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Misplaced {
    pub workspace_id: String,
    pub sourced_at: String,
}

/// Where devpod's workspaces are on this disk, and which ones are unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceLocations {
    /// Resolved source path to the workspace that opens it.
    by_path: IndexMap<PathBuf, String>,
    /// `unlocatable` is not an empty result with a note on it — see
    /// [`Unlocatable`].
    unlocatable: Vec<Unlocatable>,
    /// The same refusal made narrow, keyed by `(owner, repo)`. A workspace devpod
    /// records at a non-clone *inside one repository's clone tree* can only be
    /// confused with that repository's clones, so it disputes those and leaves
    /// every other repository prunable — which is what keeps `--prune` usable on
    /// the host devlaunch#88 describes rather than merely safe on it.
    misplaced: BTreeMap<(String, String), Misplaced>,
}

impl WorkspaceLocations {
    /// The live workspaces this command cannot place, or nothing when every one of
    /// them placed itself.
    ///
    /// A [`NonEmpty`] rather than a list, because the caller's response to it is to
    /// stop, and "stop, and here are no reasons" is not a thing to say.
    pub fn unlocatable(&self) -> Option<NonEmpty<Unlocatable>> {
        NonEmpty::of(self.unlocatable.iter().cloned())
    }

    /// The live workspace `candidate` holds the checkout for, if any.
    ///
    /// At **or under**, not equal to, and the direction matters in the only way
    /// this command's mistakes matter. `devpod up <clone>/subproject` records the
    /// subdirectory, and a clone whose subdirectory a live workspace opens is a
    /// clone that live workspace needs — deleting it takes the workspace with it.
    /// Equality answered no and deleted the parent.
    ///
    /// The containment is between two canonical paths, which is what keeps it from
    /// being the lexical prefix test the reporting surface uses:
    /// `<clone>-scratch` is not under `<clone>`, and a symlinked source has already
    /// been resolved before it gets here.
    pub fn holder(&self, candidate: &Path) -> Option<&str> {
        if let Some(held_by) = self.by_path.get(candidate) {
            return Some(held_by);
        }
        self.by_path
            .iter()
            .find(|(source, _)| source.ancestors().skip(1).any(|above| above == candidate))
            .map(|(_, held_by)| held_by.as_str())
    }

    /// The workspace disputing every clone of `(owner, repo)`, if any.
    pub(crate) fn misplaced_in(&self, owner: &str, repo: &str) -> Option<&Misplaced> {
        self.misplaced.get(&(owner.to_owned(), repo.to_owned()))
    }
}

/// Where a resolved source sits with respect to devlaunch's clone tree.
///
/// Read off the path rather than derived from an id, because on devlaunch#88's host
/// the id is what went wrong and the path is what survived: devpod's stale record
/// still says `<root>/blooop/devlaunch/<old-leaf>`, which names the repository
/// exactly even though the leaf and the workspace id match nothing any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceSite {
    /// Not in devlaunch's clone tree, so no clone answers for it.
    Outside,
    /// At or under `clone`, a directory that holds a checkout.
    InAClone { clone: PathBuf },
    /// In `(owner, repo)`'s clone tree but at no clone of it.
    InARepositoryOnly { owner: String, repo: String },
    /// In the clone tree above any repository, so it names none.
    TooShallow,
}

/// [`SourceSite`] for one resolved source.
///
/// The clone is the *third* component under the root and the source may be
/// deeper — `devpod up <clone>/subproject` is a live workspace whose source is
/// inside a clone, and the clone is what answers for it.
pub(crate) fn site_of(source: &Path, root: &Path) -> SourceSite {
    let Ok(relative) = source.strip_prefix(root) else {
        return SourceSite::Outside;
    };
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [] | [_] => SourceSite::TooShallow,
        [owner, repo] => SourceSite::InARepositoryOnly {
            owner: owner.clone(),
            repo: repo.clone(),
        },
        [owner, repo, leaf, ..] => {
            let clone = root.join(owner).join(repo).join(leaf);
            if is_populated_clone(&clone) {
                SourceSite::InAClone { clone }
            } else {
                SourceSite::InARepositoryOnly {
                    owner: owner.clone(),
                    repo: repo.clone(),
                }
            }
        }
    }
}

/// Resolve every live workspace's source to a real directory on this disk.
///
/// Both sides of the comparison this feeds are canonical, and that is the whole
/// point rather than tidiness. A cache reached through a symlink — somebody moved
/// theirs, or `/tmp` is a link on their machine — makes a lexical comparison say
/// that *no* clone is referenced, which is a total-loss bug in the one direction
/// that cannot be undone. The candidates are canonical by construction (see
/// [`prune_plan`]); this canonicalises the other side.
///
/// Three ways a workspace fails to place itself, and they are not one thing: a
/// source devlaunch cannot read at all, and a source that named a folder no
/// filesystem call will accept, both mean the workspace could be opening *any*
/// candidate and stop the command; a source that lands inside a repository's clone
/// tree on something with no `.git` in it means the workspace could be opening any
/// of *that repository's* clones, and disputes only those.
pub(crate) fn workspace_locations(workspaces: &[Workspace], root: &Path) -> WorkspaceLocations {
    let mut located = WorkspaceLocations::default();
    for workspace in workspaces {
        let places = match source_places(&workspace.source) {
            SourcePlaces::Unplaceable { payload } => {
                located.unlocatable.push(Unlocatable {
                    workspace_id: workspace.id.clone(),
                    detail: payload,
                });
                continue;
            }
            SourcePlaces::Placeable(paths) => paths,
        };
        for source in places {
            let Some(resolved) = canonical(&source) else {
                located.unlocatable.push(Unlocatable {
                    workspace_id: workspace.id.clone(),
                    detail: source,
                });
                continue;
            };
            match site_of(&resolved, root) {
                SourceSite::Outside | SourceSite::InAClone { .. } => {
                    located.by_path.insert(resolved, workspace.id.clone());
                }
                SourceSite::InARepositoryOnly { owner, repo } => {
                    located.misplaced.insert(
                        (owner, repo),
                        Misplaced {
                            workspace_id: workspace.id.clone(),
                            sourced_at: resolved.display().to_string(),
                        },
                    );
                }
                SourceSite::TooShallow => located.unlocatable.push(Unlocatable {
                    workspace_id: workspace.id.clone(),
                    detail: source,
                }),
            }
        }
    }
    located
}

/// Whether `path` is a checkout rather than a place one used to be.
///
/// `.git`'s presence, which is devlaunch#88's own published diagnostic
/// (`[ -d "$p/.git" ] || echo BROKEN`). It is what separates a devpod record that
/// still describes something from one the id-scheme change left behind — a folder
/// that is gone, or the config-only stub devpod reconstitutes from its cache,
/// neither of which any clone can be matched to.
///
/// A door this process cannot open reads as **not** a populated clone, and that is
/// the safe direction rather than the tidy one. Answering "yes" would say devpod's
/// workspace is at *this* clone and nowhere else, which leaves the repository's
/// other clones prunable; answering "no" says which clone of the repository the
/// workspace wants cannot be established, which disputes all of them and keeps
/// them.
///
/// `.git` may be a directory or a *file* (`git clone --separate-git-dir`), so this
/// asks whether anything is there rather than whether a directory is.
pub(crate) fn is_populated_clone(path: &Path) -> bool {
    std::fs::metadata(path.join(".git")).is_ok()
}

/// `path` with every symlink resolved, or nothing when it could not be followed.
///
/// `None` means "cannot tell", never "somewhere else": every caller here is
/// deciding whether a directory is referenced, and answering that from a lookup
/// that failed is how a live clone becomes an orphan.
///
/// A path that is not *there* is not a failure — this canonicalises as much of it
/// as exists and leaves the rest, which is the right answer for a workspace whose
/// source has been deleted, and there are hosts where that is most of them
/// (devlaunch#88). [`std::fs::canonicalize`] refuses such a path outright, which is
/// why this walks up to the deepest ancestor that resolves and re-appends the rest
/// — Python's `Path.resolve(strict=False)`, said out loud.
///
/// What lands in `None` is text no filesystem call will accept as a path at all: a
/// NUL byte, which a hand-edited or truncated `metadata.json` can put in a record,
/// and the empty string, which names no directory (Python read it as `Path(".")`
/// and resolved it to the working directory — the cwd-shaped answer devlaunch#224
/// is about).
pub(crate) fn canonical(path: &str) -> Option<PathBuf> {
    // A NUL byte is refused *here* rather than at the first syscall, because Rust
    // — unlike Python, where `Path(text)` raises `ValueError` before the `lstat` —
    // lets one into a `PathBuf` quite happily. Without this the walk below climbs
    // past every failing `canonicalize` to a readable ancestor and hands back a
    // path with the NUL still in it, which no later call can use either.
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    let mut trailing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = absolute;
    loop {
        if let Ok(real) = std::fs::canonicalize(&cursor) {
            let mut resolved = real;
            for part in trailing.iter().rev() {
                resolved.push(part);
            }
            return Some(resolved);
        }
        let name = cursor.file_name()?.to_owned();
        let parent = cursor.parent()?.to_path_buf();
        if parent == cursor {
            return None;
        }
        trailing.push(name);
        cursor = parent;
    }
}

/// The real directories directly under `path`, sorted, symlinks not followed.
///
/// A symlinked entry is skipped rather than followed. Following one would put a
/// candidate outside the cache entirely, and unlinking the link instead would
/// report a clone as reclaimed while it sat on another volume — the same two wrong
/// answers [`remove_tree_as_far_as_it_goes`] refuses for a symlinked root. Skipping
/// is that refusal one step earlier, and it is also what keeps every candidate's
/// path canonical without a resolve that could fail.
///
/// A directory that cannot be listed yields nothing: there is no such thing as a
/// clone this process can delete but not see, so the safe reading of a closed door
/// is that there is nothing behind it to remove.
pub(super) fn subdirectories(path: &Path) -> Vec<PathBuf> {
    let Ok(listed) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = listed
        .flatten()
        .filter(|entry| matches!(entry.file_type(), Ok(kind) if kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}
