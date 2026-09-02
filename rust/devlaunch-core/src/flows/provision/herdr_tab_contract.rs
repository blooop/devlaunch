//! What `docs/workspace-tools.md` promises about the herdr tab, checked against the
//! code that keeps it.
//!
//! The section next door about zellij is guarded because it publishes a default and
//! a price. This one is guarded because it publishes **three literals a reader acts
//! on**, and every one of them belongs to a program that is not this one:
//!
//! - `herdr tab rename`, which is herdr's command line and not dl's. A reader who
//!   wants to know what dl will run reads it here, and a herdr that renamed the
//!   subcommand would leave the page confidently wrong about a command that no
//!   longer exists.
//! - `HERDR_TAB_ID` and `HERDR_BIN_PATH`, which are herdr's exports. The page says
//!   the absence of the first is the whole of the detection, which is a claim about
//!   what dl reads: spell either differently in the code and the feature silently
//!   never fires, while the page goes on explaining a mechanism nothing runs.
//!
//! Silence is what makes them worth a test. The stage is best-effort by design and
//! reports nothing, so a wrong variable name produces no error, no warning and no
//! missing output. It produces a tab that still says `4`, which is exactly what the
//! bug looked like before the feature existed.
//!
//! The **switch** gets an assertion for a different reason: `DEVLAUNCH_NO_TITLE` is
//! documented in three places now, and the page's sentence about it is a promise
//! that opting out of one naming mechanism opts out of all of them. That is
//! [`naming_gate`](crate::flows::launch)'s doing rather than a coincidence, and a
//! reader who turns the switch off has no way to check the tab did too.
//!
//! Everything else in the section is explanation and gets no assertions, for the
//! reason [`super::lending_contract`] gives about the trip-by-trip narrative.
//!
//! The section splitter and the reflow come from that module rather than being
//! written again here, which is also why this file sits under `provision` while the
//! code it guards is in `flows::launch`: a second implementation of "the text under
//! this heading" is a second thing that can drift from the document.

use super::lending_contract::{CONTRACT_DOC, contract_doc, reflowed, section};
use crate::flows::launch::{
    HERDR_BIN_VAR, HERDR_TAB_VAR, HerdrTabRename, Host, TITLE_DISABLE_VAR, TerminalTitle,
};

/// The section carrying the whole of what this file guards.
const HERDR_TAB_HEADING: &str = "### The herdr tab, which is renamed and not written to";

/// The prose under that heading, whitespace-normalised so an assertion survives a
/// reflow of the paragraph it lands in.
fn contract() -> String {
    reflowed(&section(&contract_doc(), HERDR_TAB_HEADING))
}

/// A host in a herdr pane, with a terminal, and no opinion about titles.
fn herding() -> Host {
    Host {
        stderr_tty: true,
        herdr_tab_id: Some("w8:t7".to_owned()),
        ..Host::default()
    }
}

#[test]
fn the_page_publishes_the_command_the_stage_actually_runs() {
    // Read out of the argv rather than matched as a phrase, so the page cannot go on
    // naming `herdr tab rename` after the code stopped building it. The three words
    // are asserted as one span because that is how herdr's CLI takes them and how a
    // reader would type them.
    let rename = HerdrTabRename::from_host(&herding(), "rocker@nb1");
    let argv = rename
        .argv()
        .expect("a herdr pane with a terminal renames its tab");
    let spelled = argv[1..3].join(" ");

    assert_eq!(
        spelled, "tab rename",
        "the stage no longer runs `herdr tab rename`"
    );
    assert!(
        contract().contains(&format!("`herdr {spelled}`")),
        "{CONTRACT_DOC}'s \"{HERDR_TAB_HEADING}\" no longer names `herdr {spelled}`, \
         which is the command it tells a reader dl will run"
    );
}

#[test]
fn the_page_names_the_variables_the_detection_really_reads() {
    // The page says the absence of the tab id is the whole of the detection, so the
    // name it prints has to be the name that is read. Asserted from the constants,
    // which is what `Host::from_process` looks up.
    let page = contract();
    for named in [HERDR_TAB_VAR, HERDR_BIN_VAR] {
        assert!(
            page.contains(&format!("`{named}`")),
            "{CONTRACT_DOC}'s \"{HERDR_TAB_HEADING}\" no longer names {named}, which is what \
             the stage reads to find its tab; a page that names a different variable describes \
             a mechanism that never fires, and it fires silently"
        );
    }
}

#[test]
fn the_tab_id_is_load_bearing_and_not_decoration() {
    // The other half of the sentence above: a pane that exports no tab id must get
    // no rename at all. Without this the page's "does nothing at all when it is
    // absent" would be satisfied by a stage that ran `herdr tab rename ''`.
    let outside = Host {
        herdr_tab_id: None,
        ..herding()
    };

    assert_eq!(
        HerdrTabRename::from_host(&outside, "rocker@nb1"),
        HerdrTabRename::Off,
        "{CONTRACT_DOC} says dl does nothing at all when {HERDR_TAB_VAR} is absent"
    );
}

#[test]
fn the_page_is_right_that_one_switch_turns_all_three_names_off() {
    // "one feature and not three". The page says `DEVLAUNCH_NO_TITLE=1` takes the
    // tab with the escape, and the two are read here from one host so the assertion
    // is about the shared gate rather than about either emitter's own answer.
    let opted_out = Host {
        no_title: Some("1".to_owned()),
        ..herding()
    };

    assert_eq!(
        TerminalTitle::from_host(&opted_out, "rocker@nb1"),
        TerminalTitle::Off,
        "{TITLE_DISABLE_VAR} no longer silences the escape"
    );
    assert_eq!(
        HerdrTabRename::from_host(&opted_out, "rocker@nb1"),
        HerdrTabRename::Off,
        "{TITLE_DISABLE_VAR} no longer silences the tab rename, so {CONTRACT_DOC}'s \
         \"one feature and not three\" is now false"
    );
    assert!(
        contract().contains(&format!("`{TITLE_DISABLE_VAR}=1`")),
        "{CONTRACT_DOC}'s \"{HERDR_TAB_HEADING}\" no longer tells a reader which switch \
         turns this off"
    );
}

#[test]
fn the_page_is_right_that_the_tab_and_the_pane_get_the_one_name() {
    // The claim the whole shape exists for, held against both emitters at once. A
    // name the shared filter changes, so a second sanitiser fails this too.
    let host = herding();
    let raw = "rocker$(id)@nb1";

    let label = match HerdrTabRename::from_host(&host, raw) {
        HerdrTabRename::Run { label, .. } => label,
        HerdrTabRename::Off => panic!("a herdr pane with a terminal names its tab"),
    };
    let osc = TerminalTitle::from_host(&host, raw)
        .osc()
        .expect("the same host writes an escape")
        .to_owned();

    assert_eq!(
        osc,
        format!("\x1b]2;{label}\x07"),
        "the tab and the pane are being given different names, which is what \
         {CONTRACT_DOC} says this feature exists to prevent"
    );
}
