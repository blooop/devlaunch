//! What `docs/workspace-tools.md` promises about zellij, checked against the code
//! that keeps it (#391).
//!
//! The lending contract next door is guarded because an image author *acts* on it.
//! This section is guarded for a different reason: it publishes a **default** and a
//! **price**, and both are facts about this module that the prose cannot see change.
//! Two failures are worth a test each.
//!
//! **The polarity of the guarantee.** The page opened for a year with "Every
//! workspace `dl` opens also has zellij on `PATH`" and argued for it the way the
//! lending section argues, from the fact that nothing has to cooperate. Since #391
//! it holds for launches that asked. A sentence claiming the old default is not a
//! stale detail: it is the page telling a reader that a capability is there when it
//! is not, which is how somebody writes `zellij attach` into a script.
//!
//! **The cost table.** The stage does two things -- it bootstraps pixi and then
//! installs one package -- and the old table reported only the second. 1.70s of the
//! stage's ~2.2s is the bootstrap, and it is the term that decides whether this is
//! worth asking for, so a table that omits it is advertising a price nobody pays.
//! Asserted from [`zellij_script`] rather than from the numbers: what a test can
//! know is *which costs the stage incurs*, so that is what it asks, and the seconds
//! stay a measurement of one host that no test can re-run.
//!
//! Everything else in the section is explanation and gets no assertions, for the
//! reason `lending_contract` gives about the trip-by-trip narrative.
//!
//! The section splitter, the bullet reader and the reflow come from
//! [`super::lending_contract`] rather than being written again here: a second
//! implementation of "the text under this heading" is a second thing that can drift
//! from the document, which is the same argument `test/test_bench_doc.py` makes for
//! importing the Python one.

use super::lending_contract::{CONTRACT_DOC, bolded_spans, contract_doc, reflowed, section};
use super::{PIXI_BOOTSTRAP, ZELLIJ_VAR, ZellijSwitch, zellij_script};

/// The section carrying the whole of what this file guards. Matched on the heading
/// for `lending_contract`'s reason: the prose under it is free to be rewritten while
/// the assertions stay pointed at one span.
const ZELLIJ_HEADING: &str = "## A terminal beside the agent";

/// The subsection that publishes the switch, and the one that publishes the price.
/// Named separately from their parent because an assertion aimed at either must not
/// be satisfiable by the other.
const SWITCH_HEADING: &str = "### The one switch, and both things it does";
const COST_HEADING: &str = "### What it costs";

/// The text under `ZELLIJ_HEADING` down to its first subsection: the guarantee and
/// the argument for it, which is the span whose polarity is at stake.
///
/// Taken as the parent section minus every `###` under it, rather than as a span of
/// its own, because there is no heading between the two and a claim moved a
/// paragraph down is still the same claim.
fn opening() -> String {
    let whole = section(&contract_doc(), ZELLIJ_HEADING);
    let end = whole
        .lines()
        .position(|line| line.starts_with("### "))
        .unwrap_or(whole.lines().count());
    whole.lines().take(end).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_opening_promises_zellij_to_the_launches_that_asked_and_no_others() {
    // The polarity, read out of the switch rather than out of the sentence.
    //
    // `ZellijSwitch::requested(None)` is what a launch that set nothing gets, so it
    // is the fact the opening paragraph is a claim about.
    assert_eq!(
        ZellijSwitch::requested(None),
        ZellijSwitch::Skip,
        "the default installs zellij again; {CONTRACT_DOC}'s opening claims it does not"
    );

    let opening = reflowed(&opening());

    // The *first sentence*, and that is where the polarity lives rather than
    // anywhere in the span. It is the sentence a skimmer reads and the one the old
    // page got wrong ("Every workspace `dl` opens also has zellij on `PATH`"), and a
    // subsection-wide search would be satisfied by the paragraph three below that
    // explains the ask while the opening claim says the opposite. So the condition
    // has to be in the claim itself.
    let guarantee = opening
        .split(". ")
        .next()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        guarantee.contains("ask"),
        "{CONTRACT_DOC}'s \"{ZELLIJ_HEADING}\" opens by promising zellij unconditionally; \
         since #391 it arrives only where {ZELLIJ_VAR} asked for it. The sentence read: \
         {guarantee:?}"
    );
    assert!(
        bolded_spans(&opening)
            .iter()
            .any(|span| span.to_lowercase().contains("asking")),
        "{CONTRACT_DOC}'s \"{ZELLIJ_HEADING}\" no longer states in bold that this waits to be \
         asked for, which is the whole of the default #391 changed"
    );
    assert!(
        opening.contains(ZELLIJ_VAR),
        "{CONTRACT_DOC}'s \"{ZELLIJ_HEADING}\" no longer names {ZELLIJ_VAR}, which is the \
         only way a launch gets zellij"
    );
}

#[test]
fn the_switch_subsection_says_the_variable_installs_as_well_as_wraps() {
    // The variable's *scope*, which is what changed about it.
    //
    // It used to start a session and nothing else, and the page said so at length.
    // A reader who still believes that sets it expecting a session in a container
    // that was provisioned some other way -- and will not understand why an
    // unrelated 2.2s appeared on the launch. So the row that documents it has to
    // name both halves.
    let switch = section(&contract_doc(), SWITCH_HEADING);
    let rows: Vec<&str> = switch
        .lines()
        .filter(|line| line.starts_with("| ") && line.contains(ZELLIJ_VAR))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one table row for {ZELLIJ_VAR} under {CONTRACT_DOC}'s \
         \"{SWITCH_HEADING}\", found {} -- it was reworded, split or deleted",
        rows.len()
    );
    let row = rows[0];
    for half in ["Install", "session"] {
        assert!(
            row.contains(half),
            "{CONTRACT_DOC}'s {ZELLIJ_VAR} row does not mention {half:?}; the variable does \
             both, and a reader who believes it does one is surprised by the other"
        );
    }
}

#[test]
fn the_cost_table_carries_the_bootstrap_the_stage_really_pays() {
    // The dominant term, asserted from the script that incurs it.
    //
    // `zellij_script` bootstraps pixi before it installs anything, because the
    // container this most often lands in has no pixi -- a lend returns before the
    // tools install trip is reached, so this stage is where pixi arrives. That is
    // 1.70s of a ~2.2s stage, and a cost table that reports only the ~0.5s install
    // understates the whole thing by a factor of four.
    assert!(
        zellij_script().contains(PIXI_BOOTSTRAP),
        "the zellij stage no longer bootstraps pixi; {CONTRACT_DOC}'s cost table says it does"
    );

    let costs = reflowed(&section(&contract_doc(), COST_HEADING));
    assert!(
        costs.contains("Bootstrapping pixi"),
        "{CONTRACT_DOC}'s \"{COST_HEADING}\" no longer reports the pixi bootstrap, which is the \
         larger half of what the stage costs"
    );
    assert!(
        costs.contains("**Installing zellij**"),
        "{CONTRACT_DOC}'s \"{COST_HEADING}\" no longer reports the install itself"
    );
}
