//! The tagged derivative subtrees inside a site that has to stand
//! (devlaunch#468).
//!
//! # Why reaching inside a standing site is not the wedge it looks like
//!
//! A site stands because something about it could not be proved. Removing part
//! of it therefore looks like exactly the act principle 1 exists to stop — until
//! you ask what the standing verdict is a statement *about*. Every reason in
//! [`Standing`](super::Standing) except a claimant's is an answer about git's
//! account of the site's content: `status --porcelain` through the site's admin
//! directory, reachability from a ref, or the fact that neither could be
//! obtained. A `.pixi/envs/<name>` is outside that account **by the installer's
//! own writing** — pixi writes a `.pixi/.gitignore` of `*` and `!config.toml`,
//! so nothing under `.pixi/envs/` is in any index, in any `status` output, or
//! reachable from any commit, in any clone, ever.
//!
//! The set of bytes the site's verdict is uncertain about and the set of bytes
//! under the tag are disjoint by construction, and the construction is a file
//! somebody else wrote. The shipped contradiction is what settles it: the dirt
//! probe already reports nothing about `.pixi/envs`, so a devlaunch that refused
//! this would print *this site holds work that exists nowhere else, 0 bytes* and
//! in the same breath refuse to reclaim 5 GB of it "because we could not prove
//! it safe" — two readings of one directory in one report.
//!
//! # The gate is a declaration, and it never reads a name
//!
//! [`declared_regenerable`] is the whole of what admits a directory: the first
//! 43 bytes of `<dir>/CACHEDIR.TAG` are the Cache Directory Tagging
//! Specification's published signature (<https://bford.info/cachedir/>), which
//! the program that created the directory wrote there to say the contents are
//! regenerable and belong outside a backup.
//!
//! Measured (devlaunch#468 §2, pixi 0.77.0 / uv 0.12.5 / npm 11.18.0): rattler,
//! cargo, uv and pytest all write one; `python -m venv` and npm write none, and
//! npm writes none anywhere beneath `node_modules` either. **The same directory
//! name lands on both sides** — a `.venv` is admitted or refused depending on
//! which program made it — which is why the predicate reads a file and compares
//! no directory name at all. `.pixi` and `.pixi/envs` appear nowhere in it.
//!
//! The walk **does not descend past a tag**: the outermost tagged directory is
//! the unit, because the outer declaration covers everything inside it and
//! because descending would double-count the same bytes under R3. It also stops
//! at every site the forest holds, so a nested worktree's own derivatives are
//! found once, by that site's own pass, and attributed to it.
//!
//! # What the tag does not promise, said out loud
//!
//! pixi does not defend its own declaration. Measured: a planted `my-notes.txt`
//! and a hand-written `site-packages/mypkg` both survived `pixi install
//! --frozen` unmentioned. So the tag is a claim about the directory's *purpose*,
//! not a proof about its current contents, and the argument for removal rests on
//! the disjoint-byte-sets reading above, corroborated by the declaration and by
//! the recipe being on disk — never on "everything in there was installed".
//!
//! The sharper-looking alternative is refused with numbers. Unioning every
//! `conda-meta/*.json` `files` array and calling the rest foreign looks perfect
//! on a throwaway environment (6650 recorded, 6678 walked) and fails on a real
//! one: 11002 recorded against 12210 walked, a 1208-file delta that is the pypi
//! half recorded in `.dist-info/RECORD` plus `__pycache__` trees no installer
//! records. Roughly 10% false positives, which stands every environment.
//!
//! # A recipe, or it stands
//!
//! The tag says *regenerable*; it does not say *by what*. So a tagged directory
//! is reclaimed only when a reader on this side answers with the thing that
//! re-derives it, and a tag no reader recognises stands and is named with its
//! bytes. That is principle 1 inside the rule rather than bolted onto it, and it
//! is why [`Derivative`] carries a [`Recipe`] rather than a flag.
//!
//! One reader is implemented and it reaches ~94.5 GB of the measured 104.5. The
//! measurements behind its four cases are on devlaunch#468 §3: a lock that names
//! the environment re-derives it offline (5507 of 5507 files in 0.52 s, with
//! every proxy variable pointed at a dead port); a **stale** lock still
//! re-derives what was there, because the environment on disk was itself
//! produced from that lock; an **absent** lock re-derives nothing; and an
//! environment the lock **no longer names** is reproducible from nothing on
//! disk, so it stands with `pixi clean -e <name>` as the pointer.
//!
//! # `manifest_path` is not a field of anything here
//!
//! `conda-meta/pixi` records the manifest as an absolute path, written by
//! whoever ran the install — so for every environment installed inside a
//! container it is `/workspaces/<id>/…` and does not resolve on the host. That
//! is the same trap as a container-path worktree registration, and devlaunch#445
//! and devlaunch#446 answer it by never resolving a recorded path. The
//! constructive form of that answer is that the field does not exist to be
//! resolved: this module reads `environment_name`, which is a name, and finds
//! the lockfile by walking **up from the tag, inside the site**.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::{Blank, Inside, Place, Reason, Subject, Verdict, inside_the_clone};
use crate::flows::disk_usage::{self, DiskUsage};

/// The Cache Directory Tagging Specification's signature, all 43 bytes of it.
///
/// A file's first 43 bytes, compared as bytes. Not a prefix of a line, not a
/// trimmed string: the specification defines the signature as exactly this
/// sequence at offset 0, and anything looser admits a file that merely mentions
/// it.
const CACHEDIR_SIGNATURE: &[u8; 43] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// The file the signature lives in, named by the specification.
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

/// What pixi writes into an installed environment, and the one field read from
/// it. See the module header for why `manifest_path` is not the other one.
const PIXI_RECORD: [&str; 2] = ["conda-meta", "pixi"];
const PIXI_LOCK: &str = "pixi.lock";

/// Whether the program that created `directory` declared it regenerable.
///
/// **The one expression of "what counts as a derivative."** Every site that asks
/// the question calls this, so the plan and the acting pass cannot come to
/// disagree about the same directory — a rule written twice is the defect this
/// module is most exposed to, since the answer decides whether gigabytes go.
///
/// It reads a file and compares 43 bytes. It does not look at `directory`'s
/// name, its parent's name, or its depth, and nothing in this module supplies
/// one: the only string joined onto the path is [`CACHEDIR_TAG`], which the
/// specification fixes.
fn declared_regenerable(directory: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(directory.join(CACHEDIR_TAG)) else {
        return false;
    };
    let mut head = [0u8; CACHEDIR_SIGNATURE.len()];
    file.read_exact(&mut head).is_ok() && &head == CACHEDIR_SIGNATURE
}

/// What re-derives a tagged directory. One arm per implemented reader, matched
/// exhaustively everywhere, so a second reader is a compile error at every site
/// rather than a branch nobody notices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipe {
    /// A pixi environment, re-derived by `pixi install` from the lockfile at
    /// `lock`. The environment is named because `pixi clean -e` and the lock's
    /// own `environments:` map are both keyed on it.
    PixiEnvironment { environment: String, lock: Inside },
}

impl Recipe {
    /// What a plan line says re-derives it.
    pub fn describe(&self) -> String {
        match self {
            Self::PixiEnvironment { environment, lock } => format!(
                "a pixi environment, re-derived by `pixi install -e {environment}` from {}",
                lock.as_str()
            ),
        }
    }
}

/// Why nothing on disk re-derives a tagged directory.
///
/// Every arm stands the directory. They are separate because the words differ
/// and one of them has a pointer: an environment the lockfile no longer names is
/// `pixi clean -e <name>`'s to remove, and nothing else's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRecipe {
    /// No reader on this side recognised the directory. A `rust/target`, a
    /// `.pytest_cache`, a `uv venv` somebody `uv pip install`ed into: the tag
    /// is a claim about purpose and devlaunch has nothing that re-derives it.
    NoReaderRecognisedIt,
    /// A reader recognised it and its lockfile is not there. Measured: with the
    /// lock absent, `pixi install --frozen --offline` restores 0 files.
    LockfileAbsent,
    /// The lockfile is there and does not name this environment. Measured as a
    /// real population — add an environment, install it, drop it from the
    /// manifest and reinstall — the directory survives and pixi never mentions
    /// it again.
    LockfileDoesNotNameIt { environment: String },
    /// A record was there, it read and it named no environment. Its own arm
    /// rather than [`Self::CouldNotRead`]'s: the read succeeded, and saying it
    /// did not sends somebody to a file that is fine.
    RecordNamesNoEnvironment,
    /// A record was there and would not read.
    CouldNotRead(std::io::ErrorKind),
}

impl NoRecipe {
    /// The words the plan's standing line interpolates.
    pub fn describe(&self) -> String {
        match self {
            Self::NoReaderRecognisedIt => {
                "its creator declared it regenerable and devlaunch has no reader that \
                 re-derives it"
                    .to_owned()
            }
            Self::LockfileAbsent => {
                "there is no lockfile inside this worktree to re-derive it from".to_owned()
            }
            Self::LockfileDoesNotNameIt { environment } => format!(
                "the lockfile no longer names the environment {environment}, so nothing on \
                 disk re-derives it; `pixi clean -e {environment}` is what removes it"
            ),
            Self::RecordNamesNoEnvironment => {
                "the record beside it names no environment, so nothing on disk says what \
                 would re-derive it"
                    .to_owned()
            }
            Self::CouldNotRead(kind) => {
                format!("a record that would re-derive it could not be read ({kind})")
            }
        }
    }
}

/// A directory whose creator declared it regenerable and whose recipe is on
/// disk.
///
/// Private fields, no `Default`, and the only constructor is a read that
/// answered — the same discipline [`Proof`](super::Proof) has, for the same
/// reason: *derivable* must not be the fallthrough of a filter. The
/// `public-api.rest.txt` snapshot is where the absent constructor is pinned, and
/// `nothing_but_a_read_mints_a_derivative` is the test that says so out loud.
///
/// There is deliberately no `Option<Derivative>` anywhere. Its `None` would mean
/// both *nothing tagged here* and *tagged but not costable*, which is the
/// two-meanings-one-value shape [`Tagged`] exists to refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivative {
    at: Inside,
    bytes: DiskUsage,
    from: Recipe,
}

impl Derivative {
    /// Where inside the clone it sits.
    pub fn at(&self) -> &Inside {
        &self.at
    }

    /// What removing it frees, through
    /// [`exclusive_usage`](disk_usage::exclusive_usage) like every other figure
    /// devlaunch prints. Measured: rattler copies out of the shared package
    /// cache rather than hardlinking into the prefix, even where both are on one
    /// filesystem, so every byte of an environment is billed to its own tree and
    /// every byte comes back.
    pub fn usage(&self) -> &DiskUsage {
        &self.bytes
    }

    /// What re-derives it.
    pub fn recipe(&self) -> &Recipe {
        &self.from
    }
}

/// A tagged directory that was read, and what devlaunch concluded about it.
///
/// There is deliberately no arm for "no tag": an untagged directory is not a
/// `Tagged` at all and cannot be constructed as one. Positive space, not a
/// filtered-down negative one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tagged {
    /// A reader answered, and no claimant's reason reaches it.
    Derivable(Derivative),
    /// A reader could not cost it. Carries its bytes, so principle 2 is served
    /// by visibility where it is not served by reclamation.
    CouldNotCost {
        at: Inside,
        bytes: DiskUsage,
        why: NoRecipe,
    },
    /// A claimant's reason reaches it: somebody asserted a claim over the
    /// directory and made no distinction between its parts. Named with its
    /// bytes, never removed. See [`claims_in`].
    Claimed {
        at: Inside,
        bytes: DiskUsage,
        by: Box<Reason>,
    },
}

impl Tagged {
    /// Where inside the clone it sits, whichever arm it is.
    pub fn at(&self) -> &Inside {
        match self {
            Self::Derivable(derivative) => derivative.at(),
            Self::CouldNotCost { at, .. } | Self::Claimed { at, .. } => at,
        }
    }

    /// Its bytes, whichever arm it is. Every arm carries them, because an
    /// artifact devlaunch will not reclaim is still an artifact somebody should
    /// be told the size of.
    pub fn usage(&self) -> &DiskUsage {
        match self {
            Self::Derivable(derivative) => derivative.usage(),
            Self::CouldNotCost { bytes, .. } | Self::Claimed { bytes, .. } => bytes,
        }
    }

    /// Why it is staying, or nothing when it is going.
    pub fn standing(&self) -> Option<String> {
        match self {
            Self::Derivable(_) => None,
            Self::CouldNotCost { why, .. } => Some(why.describe()),
            Self::Claimed { by, .. } => Some(match by.as_ref() {
                Reason::Holds { losses, .. } => losses.describe(),
                Reason::CouldNotProve { blank, .. } => blank.describe(),
            }),
        }
    }

    /// The one it names when it is derivable, so a caller cannot act on an arm
    /// that was not one.
    pub fn derivable(&self) -> Option<&Derivative> {
        match self {
            Self::Derivable(derivative) => Some(derivative),
            Self::CouldNotCost { .. } | Self::Claimed { .. } => None,
        }
    }
}

/// The first reason in `standing` a **claimant** asserts, which is the only kind
/// that reaches a subtree.
///
/// **The one expression of "what counts as a claimant", and it is one call to
/// [`Reason::subject`].** That method is a wildcard-free match, so a new
/// [`Blank`](super::Blank) arm has to answer devlaunch#468 §6's question at the
/// point it is added rather than inheriting a default here — and there is no
/// second list of arms anywhere for it to drift against. Nothing else in this
/// module looks at a reason to decide whether it pins, and the fold's input is
/// therefore every reason in force rather than a pre-filtered set somebody else
/// filtered by another rule.
///
/// The fold is derived rather than chosen. Ask of each reason: is this a
/// statement about git's account of the site's content, or a statement by a
/// claimant about the directory? `Holds { Uncommitted }`, `Holds { Unpushed }`
/// and every `CouldNotProve` whose blank is about git's account are the former,
/// and they do not reach the tagged subtree because the tagged subtree was never
/// in that account — it is gitignored by the installer's own writing. A
/// `git worktree lock` and a repository lock that could not be taken are the
/// latter: somebody asserted a claim over the directory and made no distinction
/// between its parts, and a lock may mean *running right now*.
fn first_claim(standing: &[Reason]) -> Option<&Reason> {
    standing
        .iter()
        .find(|reason| reason.subject() == Subject::AClaim)
}

/// Every standing reason in force over one site's subtree: its own, and every
/// ancestor's.
///
/// Unfiltered on purpose. Which of them *pin* is [`first_claim`]'s question and
/// only its, asked where the answer is used; a list filtered here as well would
/// be the same rule written twice, in two places that can come to disagree
/// about the same directory.
///
/// An ancestor's reason reaches down because a lock on a directory is a claim
/// over everything in it — the same reading that makes a lock stand the site
/// rather than only its top level.
pub(super) fn claims_over(inherited: &[Reason], own: &Verdict) -> Vec<Reason> {
    let mut claims = inherited.to_vec();
    if let Verdict::Stands(standing) = own {
        claims.extend(standing.iter().cloned());
    }
    claims
}

/// Whether this pass costs the tagged derivatives inside the sites it stands.
///
/// `--prune` asks; `dl --ls` does not, and that is not an optimisation to be
/// tidied away later. Costing one derivative is a full walk of a site's tree
/// plus an `exclusive_usage` over a 12000-file environment, and the listing is a
/// read-only command people run casually — the same reason `site_reasons` opens
/// with a `read_dir` that fails.
///
/// The skipping arm yields no derivatives because none were asked for, and the
/// one caller that passes it discards the field rather than reading it as an
/// answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Derivatives {
    Weighed,
    NotAsked,
}

/// Every tagged directory inside `site`, with what devlaunch concluded about it.
///
/// The walk starts *below* `site` and never tests `site` itself: a site's own
/// verdict is the sweep's answer about that directory, and a tag on it would be
/// a second answer to a question already asked. It descends into everything else
/// except a tag (the unit is the outermost one) and except a place the forest
/// holds (that site's own pass covers it, and attributes it to the right path).
pub(super) fn tagged_in(
    clone: &Path,
    site: &Path,
    claims: &[Reason],
    forest: &[PathBuf],
) -> Vec<Tagged> {
    let mut found = Vec::new();
    descend(clone, site, site, claims, forest, &mut found);

    found.sort_by(|left, right| left.at().cmp(right.at()));
    found
}

fn descend(
    clone: &Path,
    site: &Path,
    at: &Path,
    claims: &[Reason],
    forest: &[PathBuf],
    into: &mut Vec<Tagged>,
) {
    let Ok(entries) = std::fs::read_dir(at) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        // Directories only, and a symlink is never followed: following one
        // walks a removal out of the tree `--prune` is scoped to.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        if forest.contains(&child) {
            // A site. It answers for itself, with its own verdict and its own
            // derivatives, attributed to its own path.
            continue;
        }
        if !declared_regenerable(&child) {
            descend(clone, site, &child, claims, forest, into);
            continue;
        }
        let Some(place) = inside_the_clone(clone, &child) else {
            continue;
        };
        into.push(classify(clone, &child, site, place, claims, forest));
        // The walk does not descend past a tag: the outer declaration covers
        // everything inside it, and descending would bill the same bytes twice.
    }
}

/// One tagged directory's verdict: the claimant fold first, then the recipe.
///
/// The order is the argument's order. A claim is about the directory as a whole
/// and admits no distinction between its parts, so it settles the question
/// before any reader is asked; only where nothing claims it does *what
/// re-derives this* become the question.
fn classify(
    clone: &Path,
    tag: &Path,
    site: &Path,
    place: Inside,
    claims: &[Reason],
    forest: &[PathBuf],
) -> Tagged {
    let bytes = disk_usage::exclusive_usage(tag);
    // A site under the tag is a claim on the directory by something the tag does
    // not speak for, and it needs no new arm: it lands in the claimant column
    // with that site's own reason. The tag is never a candidate then, whatever
    // the reader would have said.
    if let Some(nested) = forest
        .iter()
        .find(|it| it.starts_with(tag))
        .and_then(|it| inside_the_clone(clone, it))
    {
        return Tagged::Claimed {
            at: place,
            bytes,
            by: Box::new(Reason::CouldNotProve {
                at: Place::ASite(nested),
                blank: Blank::ASiteSitsInside,
            }),
        };
    }
    if let Some(claim) = first_claim(claims) {
        return Tagged::Claimed {
            at: place,
            bytes,
            by: Box::new(claim.clone()),
        };
    }
    match pixi_recipe(clone, tag, site) {
        Ok(from) => Tagged::Derivable(Derivative {
            at: place,
            bytes,
            from,
        }),
        Err(why) => Tagged::CouldNotCost {
            at: place,
            bytes,
            why,
        },
    }
}

/// The pixi reader: three reads in order, any silence yielding a [`NoRecipe`].
///
/// 1. the tag, which the caller already has;
/// 2. `conda-meta/pixi` parses and yields `environment_name` — and nothing else;
/// 3. a `pixi.lock` found by walking **up from the tag, inside the site**, whose
///    `environments:` map names that environment.
///
/// Read 3 is a walk and not a resolution of a recorded path, for the reason in
/// the module header. It stops at `site` because a lockfile above the site
/// belongs to a tree this pass is not deciding about.
fn pixi_recipe(clone: &Path, tag: &Path, site: &Path) -> Result<Recipe, NoRecipe> {
    let record = tag.join(PIXI_RECORD[0]).join(PIXI_RECORD[1]);
    let content = match std::fs::read_to_string(&record) {
        Ok(content) => content,
        // Not there at all is not a failure to read: it is this reader saying
        // the directory is not one of its own.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(NoRecipe::NoReaderRecognisedIt);
        }
        Err(error) => return Err(NoRecipe::CouldNotRead(error.kind())),
    };
    let Some(environment) = environment_name(&content) else {
        return Err(NoRecipe::RecordNamesNoEnvironment);
    };
    let Some(lock) = lockfile_above(tag, site) else {
        return Err(NoRecipe::LockfileAbsent);
    };
    let listed = match std::fs::read_to_string(&lock) {
        Ok(listed) => listed,
        Err(error) => return Err(NoRecipe::CouldNotRead(error.kind())),
    };
    if !environments_in(&listed).iter().any(|it| it == &environment) {
        return Err(NoRecipe::LockfileDoesNotNameIt { environment });
    }
    let Some(at) = inside_the_clone(clone, &lock) else {
        return Err(NoRecipe::CouldNotRead(std::io::ErrorKind::InvalidData));
    };
    Ok(Recipe::PixiEnvironment {
        environment,
        lock: at,
    })
}

/// `environment_name` out of `conda-meta/pixi`, and nothing else out of it.
///
/// Deliberately not a `serde` struct: a struct would have to name the fields it
/// ignores, and `manifest_path` is the one field this module must not be able to
/// carry. Reading one key by name is the constructive form of not having it.
fn environment_name(record: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(record).ok()?;
    let name = parsed.get("environment_name")?.as_str()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// The nearest `pixi.lock` at or above `tag`, never above `site`.
fn lockfile_above(tag: &Path, site: &Path) -> Option<PathBuf> {
    let mut at = tag;
    loop {
        let candidate = at.join(PIXI_LOCK);
        if candidate.is_file() {
            return Some(candidate);
        }
        if at == site {
            return None;
        }
        at = at.parent()?;
    }
}

/// The environment names a lockfile's top-level `environments:` map holds.
///
/// A four-line scan rather than a YAML dependency, and the shape it reads is
/// pinned by tests over real lockfile text: `environments:` at column zero, one
/// key per environment at the block's own indent, the block ending at the next
/// line in column zero. What it cannot read reads as *not named*, which stands
/// the directory.
fn environments_in(lock: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    let mut depth: Option<usize> = None;
    for line in lock.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            inside = line.trim_end() == "environments:";
            depth = None;
            continue;
        }
        if !inside {
            continue;
        }
        if indent != *depth.get_or_insert(indent) {
            continue;
        }
        let Some(name) = line.trim().strip_suffix(':') else {
            continue;
        };
        names.push(name.trim_matches(['"', '\'']).to_owned());
    }
    names
}

/// One derivative the acting pass reclaimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimedDerivative {
    pub path: PathBuf,
    /// The figure the plan measured, so what somebody is told they got back is
    /// what they said yes to.
    pub usage: DiskUsage,
}

/// One derivative the plan named that the acting pass would not reclaim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldDerivative {
    pub path: PathBuf,
    pub because: NotDerivableNow,
}

/// What the re-read said instead of *derivable*, or that it was never taken.
///
/// Three arms rather than an `Option<Tagged>`: *the tag is gone*, *the tag is
/// there and something changed about it*, and *the clone would not answer a
/// second time* are different facts, and the whole discipline of this module is
/// that they do not share a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotDerivableNow {
    /// The re-read found no tag at that place at all — it was removed, or its
    /// whole site was.
    NoTagThere,
    /// The re-read answered, and the answer was not derivable: a claim appeared,
    /// or the lockfile stopped naming it.
    Answered(Box<Tagged>),
    /// The re-read was never taken, because git would not list the clone a
    /// second time — the same refusal that withholds every going worktree in it.
    /// The plan's line is then the only account of this directory anybody has,
    /// so it is named here rather than dropped: a run whose only work was a
    /// derivative would otherwise answer a `y` with silence.
    TheCloneWouldNotAnswer,
}

impl NotDerivableNow {
    /// The words the report interpolates.
    pub fn describe(&self) -> String {
        match self {
            Self::NoTagThere => {
                "there is no longer a cache tag at that place, so nothing there declares \
                 itself regenerable"
                    .to_owned()
            }
            Self::TheCloneWouldNotAnswer => {
                "git would not list this clone's worktrees a second time, so what the plan \
                 said about it could not be put again"
                    .to_owned()
            }
            Self::Answered(tagged) => tagged.standing().unwrap_or_else(|| {
                // Unreachable: the acting pass only builds this arm from a
                // re-read that was *not* derivable, and every other arm carries
                // words. Total rather than reachable.
                "it could not be shown to be derivable a second time".to_owned()
            }),
        }
    }
}

#[cfg(test)]
mod tests;
