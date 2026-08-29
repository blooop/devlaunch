//! The lending contract the docs publish, checked against the code that keeps it.
//!
//! `docs/workspace-tools.md`'s "Tools in every workspace" section tells an image
//! author what to bake so that a launch does no provisioning work at all. That is
//! the rare piece of prose in this repository somebody *acts* on — they put it in
//! a Dockerfile — and the tree can silently invalidate it: rename the constant
//! that spells the official claude layout, move the symlink the transfer writes,
//! or stop baking the shim the section warns about, and the instructions become
//! confidently wrong with nothing failing.
//!
//! Two rules follow from that, and this file exists to obey both.
//!
//! **Assert against the code, not against the prose.** A path that merely
//! *appears* in the document would pass just as happily against a path nothing
//! checks for; asserting the path the module compares against is what makes the
//! two move together. So the layout comes from the module constant, and the
//! symlink and the PATH directory are parsed out of a script this module
//! generates.
//!
//! **Assert inside the span whose meaning is at stake.** That is the harder half,
//! and the one an earlier version of these tests got wrong: the layout assertion
//! ran against the whole section, and was satisfied by the narrative paragraph
//! that happens to mention the versions directory while the bake recipe one
//! screenful below — the six lines an author actually copies — could be rewritten
//! to name a path `dl` will never recognise with every test still green. Every
//! assertion here is therefore scoped to the subsection, and where it matters the
//! individual bullet, that carries the claim; and where the claim is a *warning*,
//! its polarity is asserted too, because a paragraph inverted to say the opposite
//! keeps every code span the old assertions looked for.
//!
//! The rest of the section — the trip-by-trip narrative, the rationale — is
//! explanation, and is left alone for the same reason most of AGENTS.md is. The one
//! exception is the set of readings a probe can produce, because that set is not
//! prose: it is an enum, and the section has to keep covering all of it.
//!
//! The changelog claim is guarded the other way round: the entry describing the
//! lend may not advertise a verification of the payload that the lend does not
//! perform.
//!
//! These tests were the Python `test_lending_doc` until the Python implementation
//! was retired (#267). They are here rather than there because every constant and every
//! generator they read is in this module: from Python they would have had to be
//! *parsed* out of this file, and a prose-drift guard that can only read the source
//! it is guarding is one rename away from silently passing.

use std::path::{Path, PathBuf};

use super::{
    CLAUDE_VERSIONS_RELPATH, HostPayload, ProbeResult, REQUIRED_TOOLS, is_official_claude,
    probe_script, transfer_script,
};

// ===========================================================================
// the documents
// ===========================================================================

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

/// The path the messages name, so a retargeted guard renames itself in all of
/// them rather than in none.
pub(super) const CONTRACT_DOC: &str = "docs/workspace-tools.md";

/// The document carrying the lending contract.
///
/// It was README.md until the README was cut back to an orientation document and
/// the provisioning sections moved to `docs/workspace-tools.md` under the same
/// three headings. What this file guards is that an image author acting on the
/// bake recipe is acting on paths `dl` really looks for, which is a property of
/// the prose wherever it is published, not of the filename.
pub(super) fn contract_doc() -> String {
    read(&repo_root().join(CONTRACT_DOC))
}

pub(super) fn changelog() -> String {
    read(&repo_root().join("CHANGELOG.md"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// This repo's own devcontainer feature installer, as the bake recipe's warning names it.
const CLAUDE_FEATURE_INSTALLER: &str = ".devcontainer/claude-code/install.sh";

// Matched on the heading rather than on any phrase under it, so the prose is free
// to be rewritten while the assertions stay pointed at one span. The recipe and
// the non-goals are named separately from their parent section precisely because
// an assertion aimed at either must not be satisfiable by the other.
const NARRATIVE_HEADING: &str = "### How they get there";
const BAKE_HEADING: &str = "### What to bake so a launch does no work at all";
const NON_GOALS_HEADING: &str = "### What this deliberately does not do";

/// The text under `heading`, up to the next heading of its own level or above.
///
/// Level-aware, so a `###` subsection ends where the next `###` begins rather than
/// swallowing the rest of its `##` parent — that difference is what lets an
/// assertion be aimed at the recipe instead of at everything near it.
///
/// The heading must occur exactly once and as a whole line: a renamed or deleted
/// heading is the likeliest way these tests break and has to say so rather than
/// surface as an empty string, and a substring search would just as happily match
/// the same words quoted inside a code fence.
pub(super) fn section(document: &str, heading: &str) -> String {
    let level = heading.len() - heading.trim_start_matches('#').len();
    let starts: Vec<usize> = document
        .lines()
        .enumerate()
        .filter(|(_, line)| *line == heading)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "the document has {} lines exactly equal to {heading:?}; expected one \
         -- it was renamed, removed or duplicated",
        starts.len()
    );

    let lines: Vec<&str> = document.lines().collect();
    let body = &lines[starts[0] + 1..];
    let end = body.iter().position(|line| is_heading_at_most(line, level));
    body[..end.unwrap_or(body.len())].join("\n")
}

/// Whether `line` opens a heading of level `level` or shallower.
fn is_heading_at_most(line: &str, level: usize) -> bool {
    let hashes = line.len() - line.trim_start_matches('#').len();
    hashes >= 1 && hashes <= level && line[hashes..].starts_with(' ')
}

/// The top-level `- ` bullets of `text`, each with its continuation lines.
///
/// Assertions land on a single bullet rather than on the subsection because a
/// subsection-wide assertion is satisfied by any sentence in it — including a
/// parenthetical listing the guarded spans next to a recipe that says something
/// else entirely.
pub(super) fn bullets(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut in_bullet = false;
    for line in text.lines() {
        if line.starts_with("- ") {
            found.push(line.to_owned());
            in_bullet = true;
        } else if in_bullet && line.starts_with("  ") {
            let last = found.last_mut().expect("a bullet to continue");
            last.push('\n');
            last.push_str(line);
        } else if !line.trim().is_empty() {
            in_bullet = false;
        }
    }
    found
}

/// The one bullet of `text` whose bolded lead is `term`.
pub(super) fn bullet_naming(text: &str, term: &str) -> String {
    let lead = format!("- **{term}**");
    let matching: Vec<String> = bullets(text)
        .into_iter()
        .filter(|bullet| bullet.starts_with(&lead))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one bullet starting {lead:?}, found {}",
        matching.len()
    );
    matching.into_iter().next().expect("the matching bullet")
}

fn bake_recipe() -> String {
    section(&contract_doc(), BAKE_HEADING)
}

// ===========================================================================
// the script a real lend runs
// ===========================================================================

/// The script a real lend runs, generated the way a lend generates it.
///
/// The version and the host paths are arbitrary because nothing asserted here
/// depends on them; what matters is that the paths below come out of this
/// module's own string assembly, not out of this file.
fn a_transfer_script() -> String {
    transfer_script(&HostPayload {
        claude_version: "1.2.3".to_owned(),
        claude_binary: PathBuf::from("/host/claude"),
        gh_binary: PathBuf::from("/host/gh"),
    })
}

/// Where a lend puts the `claude` a login shell will find, as `~/...`.
fn lent_claude_symlink() -> String {
    let script = a_transfer_script();
    let line = script
        .lines()
        .find(|line| line.starts_with("ln -sfn "))
        .expect("the transfer script no longer creates a symlink");
    // `ln -sfn "$HOME/<target>" "$HOME/<link>"` — the link is the second quoted
    // argument, and it is the one being described.
    let quoted: Vec<&str> = line.split('"').collect();
    let link = quoted
        .get(3)
        .expect("the ln line no longer has two quoted arguments");
    let relative = link
        .strip_prefix("$HOME/")
        .expect("the claude symlink is no longer created under $HOME");
    format!("~/{relative}")
}

/// The directory a lend puts in front of the login PATH, as `~/...`.
fn prepended_path_dir() -> String {
    let script = a_transfer_script();
    let opener = r#"export PATH="$HOME/"#;
    let start = script
        .find(opener)
        .expect("the transfer script no longer prepends a directory under $HOME to PATH")
        + opener.len();
    let rest = &script[start..];
    let end = rest
        .find(":$PATH\"")
        .expect("the PATH line no longer prepends to $PATH");
    format!("~/{}", &rest[..end])
}

/// Whether the generated transfer really runs the binaries before moving any.
fn transfer_gates_on_execution() -> bool {
    let script = a_transfer_script();
    let lines: Vec<&str> = script.lines().collect();
    let version_checks: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("--version"))
        .map(|(index, _)| index)
        .collect();
    let moves: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("mv -f "))
        .map(|(index, _)| index)
        .collect();
    match (version_checks.iter().max(), moves.iter().min()) {
        (Some(last_check), Some(first_move)) => last_check < first_move,
        _ => false,
    }
}

/// `text` with every run of whitespace collapsed to one space.
///
/// The documents are hard-wrapped, so a span and the verb that governs it are one
/// reflow away from being on separate lines — and that is not a change of meaning.
pub(super) fn reflowed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `needle` occurs in `haystack` at a word start, case-insensitively.
///
/// The claims banned below are whole words; without the boundary, "signed" would
/// be found inside "cosigned" and "attest" inside a word nobody wrote.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    haystack.match_indices(&needle).any(|(at, _)| {
        at == 0
            || !haystack[..at]
                .chars()
                .next_back()
                .is_some_and(|before| before.is_alphanumeric() || before == '_')
    })
}

// ===========================================================================
// the narrative
// ===========================================================================

#[test]
fn the_narrative_explains_every_reading_a_probe_can_produce() {
    // The three outcomes a launch branches on, named where they are explained.
    //
    // The narrative is otherwise explanation and gets no assertions -- but the set
    // of readings is not prose, it is `ProbeResult`, and a fourth state added to
    // that enum without a paragraph saying what it costs a reader leaves the
    // section describing a flow that no longer exists.
    let narrative = section(&contract_doc(), NARRATIVE_HEADING);
    for state in ProbeResult::ALL {
        let word = state.word();
        assert!(
            narrative.contains(&format!("**{word}**")),
            "{CONTRACT_DOC} no longer explains the {word:?} reading a probe can produce"
        );
    }
}

// ===========================================================================
// the bake recipe
// ===========================================================================

#[test]
fn the_recipe_gives_every_tool_a_workspace_is_promised_its_own_instruction() {
    // A tool this module provisions but the recipe never tells you to bake.
    //
    // One bullet per tool, not one mention per section: a recipe that names both
    // tools in passing and then says "ask your platform team" has told an image
    // author nothing, and that is a shape a token count cannot tell from a
    // contract. Each bullet has to name a place a *login shell* will find the tool
    // -- the login PATH, or a path under `~` reached from it -- because that is the
    // only question the probe asks, and a bullet saying `dl` will find it anywhere
    // on the filesystem is an instruction that costs the reader the lend.
    let probe = probe_script();
    let recipe = bake_recipe();
    for tool in &REQUIRED_TOOLS {
        let command = tool.command;
        assert!(
            probe.contains(&format!("command -v {command}")),
            "the probe no longer resolves {command} by name, so {CONTRACT_DOC}'s recipe may be \
             describing the wrong precondition"
        );
        let bullet = bullet_naming(&recipe, &format!("`{command}`"));
        assert!(
            bullet.contains("login PATH") || bullet.contains("~/"),
            "{CONTRACT_DOC}'s bake recipe names `{command}` without saying where a login shell \
             will find it"
        );
    }
}

#[test]
fn the_recipe_names_the_claude_layout_dl_actually_looks_for() {
    // The one path an image author reproduces, taken from the module.
    //
    // Asserted inside the `claude` bullet of the recipe, in the versioned form an
    // author copies, and with its closing backtick: the section's narrative names
    // the versions directory too, so a section-wide assertion is satisfied while
    // the recipe says something else -- and a longer path that merely starts with
    // the right one (`.../<version>/bin/claude`, which the host refuses) contains
    // the unclosed span but not the closed one.
    let expected = format!("`~/{CLAUDE_VERSIONS_RELPATH}/<version>`");
    assert!(
        bullet_naming(&bake_recipe(), "`claude`").contains(&expected),
        "{CONTRACT_DOC}'s bake recipe no longer tells an image author to bake \
         claude into {expected}"
    );
}

#[test]
fn the_recipe_says_the_binary_is_a_direct_child_and_the_host_agrees() {
    // Both halves of "anything deeper does not count", stated and enforced.
    //
    // `is_official_claude` is a parent-equality test, so a binary one directory
    // deeper is refused -- exactly the `versions/latest/bin/claude` shape a
    // downloader parks there. An author who is not told that builds an image that
    // reads *lendable* forever, so the recipe has to say it and the predicate has
    // to keep meaning it.
    let home = "/containers/workspace-home";
    let versions = format!("{home}/{CLAUDE_VERSIONS_RELPATH}");
    assert!(
        is_official_claude(&versions, &format!("{versions}/1.2.3")),
        "the layout {CONTRACT_DOC} tells image authors to bake is no longer recognised"
    );
    assert!(
        !is_official_claude(&versions, &format!("{versions}/1.2.3/bin/claude")),
        "a binary nested under the versions directory now counts; the recipe says it does not"
    );
    assert!(
        bullet_naming(&bake_recipe(), "`claude`").contains("direct child"),
        "{CONTRACT_DOC}'s bake recipe no longer says the binary must be a direct child of the \
         versions directory, which is what dl requires"
    );
}

#[test]
fn the_recipe_names_the_symlink_a_lend_creates() {
    // The second half of the layout: the link, not just the versioned binary.
    let expected = format!("`{}`", lent_claude_symlink());
    assert!(
        bullet_naming(&bake_recipe(), "`claude`").contains(&expected),
        "{CONTRACT_DOC}'s bake recipe no longer names {expected}, the symlink a lend writes"
    );
}

#[test]
fn the_recipe_states_the_login_path_precondition_the_symlink_depends_on() {
    // The precondition that decides whether a perfectly baked image is seen at all.
    //
    // The probe resolves the tools by name under a login shell, so the directory
    // the `claude` symlink lives in has to be on that PATH; an image that satisfies
    // every other bullet and leaves it off is read as *absent* and pays the whole
    // lend. Taken from the symlink the transfer writes, because that is the relation
    // being asserted -- this directory matters *because* it is where the link is --
    // and given its own bullet because the requirement is not the symlink's: the
    // symlink's path contains this directory, so the bullet naming the symlink would
    // satisfy any assertion that only looked for the span.
    let symlink = lent_claude_symlink();
    let directory = symlink
        .rsplit_once('/')
        .expect("the claude symlink has no parent directory")
        .0;
    // `bullet_naming` asserts there is exactly one such bullet; the call is the
    // assertion.
    let _ = bullet_naming(&bake_recipe(), &format!("`{directory}` on the login PATH"));
}

#[test]
fn the_recipe_says_a_lend_puts_that_directory_in_front_of_the_login_path() {
    // The PATH consequence documented as intended behaviour, not left to be found.
    //
    // It is what makes a lent binary shadow a baked shim from then on, and it is a
    // separate claim from the precondition above -- the same directory, said about
    // the lend rather than about the image -- so it is asserted against the sentence
    // that makes it rather than against the span, which by now appears in two other
    // places in the recipe.
    let prepended = prepended_path_dir();
    let expected = format!("lend prepends `{prepended}`");
    assert!(
        reflowed(&bake_recipe()).contains(&expected),
        "{CONTRACT_DOC}'s bake recipe no longer says a lend prepends `{prepended}` \
         to the login PATH"
    );
}

#[test]
fn the_recipe_warns_that_this_repos_own_feature_does_not_satisfy_it() {
    // The warning, its polarity, and the thing it warns about, all three.
    //
    // The recipe tells the reader that an image built from this repo's own
    // devcontainer feature does *not* meet the contract. Naming the installer is
    // not enough -- a paragraph rewritten to say the feature already satisfies the
    // contract names it just as happily, and that inversion is the one failure mode
    // a reader is actually harmed by. So the denial itself is asserted, and so is
    // its premise: the day the feature installs the official layout instead, this
    // fails and the paragraph goes rather than quietly misleading people.
    let installer = repo_root().join(CLAUDE_FEATURE_INSTALLER);
    assert!(
        installer.is_file(),
        "{CONTRACT_DOC}'s bake recipe points at {CLAUDE_FEATURE_INSTALLER}"
    );
    assert!(
        read(&installer).contains("claude-shim"),
        "{CLAUDE_FEATURE_INSTALLER} no longer installs a shim; \
         {CONTRACT_DOC}'s warning about it is stale"
    );

    let recipe = bake_recipe();
    assert!(
        recipe.contains(CLAUDE_FEATURE_INSTALLER),
        "{CONTRACT_DOC}'s bake recipe no longer warns that this repo's own feature bakes a shim"
    );
    assert!(
        bolded_spans(&recipe)
            .iter()
            .any(|span| contains_word(span, "bakes a shim")),
        "{CONTRACT_DOC}'s bake recipe no longer states in bold that this repo's \
         own feature bakes a shim"
    );
    assert!(
        reflowed(&recipe).contains("does *not* meet the contract"),
        "{CONTRACT_DOC}'s bake recipe no longer denies that an image built from this repo's own \
         feature meets the contract"
    );
}

/// Every `**bold**` span in `text`, reflowed so a hard-wrapped span reads as one.
pub(super) fn bolded_spans(text: &str) -> Vec<String> {
    let flat = reflowed(text);
    let mut spans = Vec::new();
    let mut rest = flat.as_str();
    while let Some(open) = rest.find("**") {
        rest = &rest[open + 2..];
        match rest.find("**") {
            Some(close) => {
                spans.push(rest[..close].to_owned());
                rest = &rest[close + 2..];
            }
            None => break,
        }
    }
    spans
}

#[test]
fn the_shim_the_recipe_warns_about_is_not_the_official_layout() {
    // The recipe's premise, asked of the predicate that decides it for real.
    //
    // "A shim does not count" is the whole reason the contract is worth writing
    // down, and it is a claim about `is_official_claude` -- the one relation both
    // ends of the pipe are read through. A home that is not under `/home` keeps
    // this free of any machine's real layout.
    let home = "/containers/workspace-home";
    let versions = format!("{home}/{CLAUDE_VERSIONS_RELPATH}");
    let shim = format!("{home}/.pixi/envs/claude-shim/bin/claude");
    assert!(
        !is_official_claude(&versions, &shim),
        "a pixi shim now counts as the official layout; {CONTRACT_DOC}'s contract says it does not"
    );
}

// ===========================================================================
// the non-goals
// ===========================================================================

/// What #141 settled that the section has to keep saying, because a reader plans
/// around both: nothing here is a claim the code can invalidate, which is exactly
/// why they had no guard at all and could be deleted wholesale in silence.
const DOCUMENTED_NON_GOALS: [&str; 2] = ["No per-tool transfer.", "No version sync."];

#[test]
fn the_section_keeps_stating_the_non_goals_it_committed_to() {
    // Each accepted non-goal survives as its own bullet, in the bold a reader skims.
    //
    // An image author sizing a build plans around these: that a half-provisioned
    // image is sent both tools, and that a `claude` already there is never upgraded.
    // Deleting them costs nothing anywhere else in the tree, which is the only
    // reason this test exists.
    let non_goals = section(&contract_doc(), NON_GOALS_HEADING);
    for non_goal in DOCUMENTED_NON_GOALS {
        // `bullet_naming` asserts there is exactly one such bullet, so the call is
        // the assertion: it panics naming the bullet it could not find.
        let _ = bullet_naming(&non_goals, non_goal);
    }
}

// ===========================================================================
// the changelog entry
// ===========================================================================

/// How the entry about the lend introduces itself. Located by this lead across the
/// whole changelog rather than under a section heading, and scoped to the one entry
/// rather than to the section holding it.
///
/// Not under `## [Unreleased]`, which is where the entry was written and where an
/// earlier version of this guard looked for it: cutting a release inserts a
/// `## [<version>] - <date>` heading *above* the existing text, so 0.0.24 sealed
/// this entry into a released section without changing a word of it. A guard
/// anchored on the unreleased heading stops guarding anything the first time the
/// project releases — and stops silently, because the entry it watches has not been
/// edited, only re-parented. What is being guarded is a property of this entry, and
/// the entry keeps it wherever it lives.
///
/// Scoped to the one entry because the claim being guarded is this entry's: another
/// entry is entitled to talk about signatures and checksums, and a guard that failed
/// on it would be a guard people delete.
const LEND_ENTRY_LEAD: &str = "A cold container is lent the host's own";

/// The vocabulary of a claim about the payload's *bytes* — the class of check a
/// reader hears as "corruption or tampering would have been caught". The lend
/// performs none of it: its only gate is execution, and the entry said
/// "checksum-verified" for months. Banning the one word it happened to use would
/// leave every equivalent claim ("GPG-signature-verified", "hash-checked")
/// available, so the class is what is named here. The day a lend really does verify
/// bytes, this list is edited in the same change — deliberately, by somebody
/// re-reading what the entry now promises.
const UNPERFORMED_INTEGRITY_CLAIMS: [&str; 10] = [
    "checksum",
    "sha256",
    "sha-256",
    "md5",
    "blake2",
    "signature",
    "signed",
    "gpg",
    "cryptograph",
    "attest",
];

/// The changelog entry that describes the lend, wherever it now lives.
///
/// Required to be unique across the document, so that an entry which was reworded,
/// split or duplicated fails here saying so rather than matching nothing and
/// leaving every assertion below vacuously true.
fn lend_entry() -> String {
    let document = changelog();
    let matching: Vec<String> = bullets(&document)
        .into_iter()
        .filter(|entry| reflowed(entry).contains(LEND_ENTRY_LEAD))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one changelog entry containing {LEND_ENTRY_LEAD:?}, found {} \
         -- it was reworded, split or deleted",
        matching.len()
    );
    matching.into_iter().next().expect("the lend entry")
}

#[test]
fn the_changelog_entry_describes_the_gate_the_transfer_actually_has() {
    // The positive half: what the entry says happened is what the script does.
    //
    // The entry earns the word "proved" only while the generated script really runs
    // both lent binaries and moves nothing until they have answered. Read out of the
    // script rather than trusted, so reordering the gate in code turns this red
    // instead of leaving the changelog describing a version of the transfer that no
    // longer exists.
    assert!(
        transfer_gates_on_execution(),
        "the transfer no longer runs the lent binaries before moving them into place; \
         the changelog entry says it does"
    );
    assert!(
        reflowed(&lend_entry()).contains("proved to run in a staging directory"),
        "the lend entry no longer describes the staging gate, which is the only \
         verification the lend performs"
    );
}

#[test]
fn the_changelog_entry_advertises_no_integrity_check_the_lend_does_not_perform() {
    // The negative half: no check on the bytes may be advertised, in any wording.
    //
    // Stated as a failing assertion and never as a skip. An earlier version of this
    // test stood down whenever the word appeared anywhere under `devlaunch/` --
    // including in a comment saying no checksum is computed -- and a disarmed guard
    // is how a broken run looks clean; `test/fixtures/e2e_guard.py` writes the
    // repo's rule down. There is nothing here this test needs and cannot get.
    let entry = lend_entry();
    for claim in UNPERFORMED_INTEGRITY_CLAIMS {
        assert!(
            !contains_word(&entry, claim),
            "the lend entry advertises {claim:?}; the lend verifies nothing about the \
             bytes it sends -- its only gate is running both binaries in a staging directory"
        );
    }
}
