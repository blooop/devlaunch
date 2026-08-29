//! The in-process fake, driven over the shared conformance corpus.
//!
//! There are two hand-written fake devpods in this repo, and testing each one
//! against itself is exactly how they drifted: `delete --ignore-not-found`
//! refused here long after the shim was fixed for it, because nothing compared
//! the two. `test/fixtures/devpod/conformance.json` holds the expectations once
//! — real devpod v0.26.1's behaviour, with per-row provenance — and this test
//! drives [`DevpodMachine`] over every row while `test/unit/` drives the shim
//! program over the same file.
//!
//! Both drivers hold the corpus's invariants, rather than one holding them for
//! both: the roll call of row ids, the shape the named rows keep, and every row's
//! provenance line. The mutation that made that necessary is deleting a row —
//! #310's own regression row went, and both suites stayed green, because the
//! guard asked only whether certain flag *names* appeared somewhere across the
//! file and a sibling row still mentioned the one that mattered.
//!
//! Rows assert exit code and workspace state, and nothing about stdout: real
//! devpod answers a missing-and-ignored delete with a timestamped log line no
//! fake spells, so pinning text would pin a fake's invention. The output shapes
//! that matter are pinned against recorded fixtures in each fake's own tests.

use super::*;
use devlaunch_runner::Exit;
use serde::Deserialize;

/// The corpus, read at compile time from the directory both suites share.
const CORPUS: &str = include_str!("../../../../test/fixtures/devpod/conformance.json");

/// The other fake's source, read the same way so its flag tables can be compared
/// with this one's. Nothing here executes it; see [`the_two_fakes_agree_on_which
/// _flags_take_a_value`].
const SHIM: &str = include_str!("../../../../test/fixtures/devpod_shim.py");

/// Every row this driver expects to find in the corpus, by `id`.
///
/// This is the roll call, and it is the answer to a corpus row being deleted
/// with every suite still green — which is what happened when the guard checked
/// only that certain flag *names* appeared somewhere across all rows. The
/// regression row for #310 could go, because its sibling still mentioned the
/// flag. A missing id now fails by name, here and in the pytest driver, which
/// holds the same list.
const ROLL_CALL: &[&str] = &[
    "delete-missing-with-ignore-not-found",
    "delete-missing-with-leading-ignore-not-found",
    "delete-missing-without-the-flag",
    "delete-removes-the-workspace",
    "delete-present-with-ignore-not-found",
    "stop-missing-workspace",
    "stop-running-workspace",
    "up-full-production-argv",
    "up-derives-id-with-trailing-flags",
    "up-init-env-before-source",
    "up-mount-before-source",
    "up-dotfiles-script-before-source",
    "up-dotfiles-script-env-before-source",
    "up-workspace-env-file-before-source",
    "up-restarts-a-stopped-workspace",
    "ssh-trailing-workdir-starts-workspace",
    "ssh-workdir-before-workspace",
    "ssh-workdir-and-command",
    "ssh-missing-workspace",
    "status-missing-workspace",
    "status-leaves-the-workspace-alone",
    "list-on-an-empty-machine",
    "list-leaves-workspaces-alone",
    "unknown-command",
];

/// Where a flag sits relative to its subcommand's positional. Cobra takes a
/// value flag on either side, and only the leading position tells a value flag
/// from a bare one — read a value flag as bare there and its value becomes the
/// positional.
#[derive(Clone, Copy, Debug)]
enum Position {
    BeforeThePositional,
    AfterThePositional,
}

/// The shape each named row has to keep: which subcommand, which flag, and which
/// side of the positional it sits on. Keyed by row id, so the guard is about
/// *that row* rather than about the flag appearing anywhere in the file.
const REQUIRED_SHAPES: &[(&str, &str, &str, Position)] = &[
    (
        "delete-missing-with-ignore-not-found",
        "delete",
        "--ignore-not-found",
        Position::AfterThePositional,
    ),
    (
        "delete-missing-with-leading-ignore-not-found",
        "delete",
        "--ignore-not-found",
        Position::BeforeThePositional,
    ),
    (
        "up-init-env-before-source",
        "up",
        "--init-env",
        Position::BeforeThePositional,
    ),
    (
        "up-mount-before-source",
        "up",
        "--mount",
        Position::BeforeThePositional,
    ),
    (
        "up-dotfiles-script-before-source",
        "up",
        "--dotfiles-script",
        Position::BeforeThePositional,
    ),
    (
        "up-dotfiles-script-env-before-source",
        "up",
        "--dotfiles-script-env",
        Position::BeforeThePositional,
    ),
    (
        "up-workspace-env-file-before-source",
        "up",
        "--workspace-env-file",
        Position::BeforeThePositional,
    ),
    (
        "ssh-workdir-before-workspace",
        "ssh",
        "--workdir",
        Position::BeforeThePositional,
    ),
    (
        "ssh-trailing-workdir-starts-workspace",
        "ssh",
        "--workdir",
        Position::AfterThePositional,
    ),
];

#[derive(Debug, Deserialize)]
struct Corpus {
    rows: Vec<Row>,
}

/// One row: seeded state, argv, expected exit, expected workspaces afterwards.
#[derive(Debug, Deserialize)]
struct Row {
    id: String,
    name: String,
    /// Why the row is here, and how real devpod's behaviour was established.
    /// Read only to keep the corpus honest — see [`every_row_says_how_it_was_verified`].
    #[allow(dead_code)]
    why: String,
    verified: String,
    given: Vec<Seed>,
    argv: Vec<String>,
    exit: i32,
    then: Vec<Expected>,
}

#[derive(Debug, Deserialize)]
struct Seed {
    id: String,
    source: String,
    state: StateWord,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Expected {
    id: String,
    state: StateWord,
}

/// devpod's own two words for whether a container is up, as the corpus spells
/// them.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
enum StateWord {
    Running,
    Stopped,
}

impl From<StateWord> for WorkspaceState {
    fn from(word: StateWord) -> Self {
        match word {
            StateWord::Running => Self::Running,
            StateWord::Stopped => Self::Stopped,
        }
    }
}

impl From<WorkspaceState> for StateWord {
    fn from(state: WorkspaceState) -> Self {
        match state {
            WorkspaceState::Running => Self::Running,
            WorkspaceState::Stopped => Self::Stopped,
        }
    }
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("the conformance corpus parses")
}

fn exit_code_of(response: &Response) -> i32 {
    match response {
        Response::Ran {
            exit: Exit::Code(code),
            ..
        } => *code,
        other => panic!("a fake devpod always exits with a code, got {other:?}"),
    }
}

#[test]
fn every_row_of_the_corpus_holds_for_the_in_process_fake() {
    let mut consumed = 0;
    for row in corpus().rows {
        consumed += 1;
        let mut machine = DevpodMachine::new();
        for seed in &row.given {
            machine.insert(FakeWorkspace::new(
                seed.id.clone(),
                Source::classify(&seed.source),
                seed.state.into(),
            ));
        }

        let response = machine.answer(&row.argv);

        assert_eq!(
            exit_code_of(&response),
            row.exit,
            "exit code for row {:?} (argv {:?})\n  {}",
            row.name,
            row.argv,
            row.verified
        );

        let mut after: Vec<Expected> = machine
            .ids()
            .into_iter()
            .map(|id| Expected {
                state: machine
                    .state_of(&id)
                    .expect("a listed workspace has a state")
                    .into(),
                id,
            })
            .collect();
        after.sort_by(|left, right| left.id.cmp(&right.id));

        assert_eq!(
            after, row.then,
            "workspaces after row {:?} (argv {:?})\n  {}",
            row.name, row.argv, row.verified
        );
    }

    // The loop above is only a guard over the rows it was handed, so say how many
    // that was. A corpus that lost rows still runs clean otherwise.
    assert_eq!(
        consumed,
        ROLL_CALL.len(),
        "this driver ran {consumed} rows and the roll call names {}",
        ROLL_CALL.len()
    );
}

#[test]
fn the_corpus_answers_the_roll_call() {
    // Aggregate checks let a row leave quietly: the old guard asked whether
    // certain flag names appeared anywhere across the rows, so deleting #310's
    // own regression row kept every suite green — its sibling still mentioned the
    // flag. Identity is what has to be guarded, so the ids are named here and
    // named again on the pytest side.
    let rows = corpus().rows;
    let found: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();

    let missing: Vec<&&str> = ROLL_CALL
        .iter()
        .filter(|id| !found.contains(*id))
        .collect();
    assert!(
        missing.is_empty(),
        "the corpus has lost rows the roll call names: {missing:?}"
    );

    let unexpected: Vec<&&str> = found
        .iter()
        .filter(|id| !ROLL_CALL.contains(*id))
        .collect();
    assert!(
        unexpected.is_empty(),
        "the corpus carries rows this driver does not name: {unexpected:?} — \
         a new row is added to the roll call in both drivers"
    );

    let mut seen: Vec<&str> = found.clone();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(
        seen.len(),
        before,
        "two corpus rows share an id, so one of them is not guarded by name"
    );
}

#[test]
fn the_rows_the_decision_named_keep_their_shape() {
    // The minimum set #309 committed to: the drift that motivated the corpus,
    // every `up` value flag production sends that both fakes mis-parsed, and
    // ssh's `--workdir`. Bound to row ids rather than to the file as a whole, so
    // moving a flag out of the row that is supposed to carry it fails even when
    // some other row still mentions it.
    let rows = corpus().rows;
    for (id, verb, flag, position) in REQUIRED_SHAPES {
        let row = rows
            .iter()
            .find(|row| row.id == *id)
            .unwrap_or_else(|| panic!("no corpus row with id {id:?}"));
        assert_eq!(
            row.argv.first().map(String::as_str),
            Some(*verb),
            "row {id:?} is supposed to exercise {verb}"
        );
        // Both shapes are read off argv[1], which is the subcommand's first word:
        // either the flag leads and the positional follows its value, or the
        // positional leads and the flag trails. Deciding it that way keeps the
        // guard clear of the value-flag tables, which are the thing under test.
        let leads = row.argv.get(1).map(String::as_str);
        match position {
            Position::BeforeThePositional => assert_eq!(
                leads,
                Some(*flag),
                "row {id:?} must put {flag} ahead of {verb}'s positional, which is \
                 the shape that tells a value flag from a bare one: {:?}",
                row.argv
            ),
            Position::AfterThePositional => {
                assert!(
                    leads.is_some_and(|arg| !arg.starts_with('-')),
                    "row {id:?} must lead with {verb}'s positional: {:?}",
                    row.argv
                );
                assert!(
                    row.argv[2..].iter().any(|arg| arg == flag),
                    "row {id:?} no longer passes {flag} after {verb}'s positional: {:?}",
                    row.argv
                );
            }
        }
    }
}

#[test]
fn every_row_says_how_it_was_verified() {
    // The corpus is only worth its keep if a row's provenance travels with it: a
    // behaviour measured against the real binary and one inherited from the two
    // fakes agreeing are different claims, and the drift got in by collapsing
    // them. So every row has to say which it is, in words. The pytest driver runs
    // this too — an invariant enforced on one side of a file two suites edit is
    // an invariant with a hole in it.
    for row in corpus().rows {
        let verified = row.verified.to_lowercase();
        assert!(
            verified.starts_with("measured") || verified.starts_with("unverified"),
            "row {:?} must open its `verified` with `measured` or `unverified`, \
             said {:?}",
            row.name,
            row.verified
        );
        assert!(
            !row.why.trim().is_empty(),
            "row {:?} must say why it is here",
            row.name
        );
        assert!(
            !row.name.trim().is_empty(),
            "row {:?} must have a name to fail under",
            row.id
        );
        assert!(
            !row.id.is_empty()
                && row
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "row id {:?} must be lowercase, digits and dashes, so it reads the \
             same in both drivers",
            row.id
        );
    }
}

#[test]
fn the_two_fakes_agree_on_which_flags_take_a_value() {
    // The tables that say which flags consume the next argv element are written
    // out twice, once per fake, and nothing compared them: dropping a flag from
    // one left every suite green, because only ~14 of the ~44 names have a corpus
    // row to catch it behaviourally. This is the other half of the corpus — the
    // corpus proves the two fakes agree on the calls it covers, and this proves
    // they agree on the tables that decide the rest.
    for (ours, theirs, table) in [
        (GLOBAL_VALUE_FLAGS, "_GLOBAL_VALUE_FLAGS", "the globals"),
        (UP_VALUE_FLAGS, "_UP_VALUE_FLAGS", "up"),
        (SSH_VALUE_FLAGS, "_SSH_VALUE_FLAGS", "ssh"),
        (DELETE_VALUE_FLAGS, "_DELETE_VALUE_FLAGS", "delete"),
        (STATUS_VALUE_FLAGS, "_STATUS_VALUE_FLAGS", "status"),
    ] {
        let mut mine: Vec<&str> = ours.to_vec();
        mine.sort_unstable();
        let mut shim = shim_table(theirs);
        shim.sort_unstable();
        assert_eq!(
            mine, shim,
            "the two fakes disagree on which {table} flags take a value; a flag in \
             one table and not the other is read as bare by one fake, which makes \
             its value that call's positional"
        );
    }

    // `stop` has no flags of its own, which no list of names can express.
    assert!(STOP_VALUE_FLAGS.is_empty());
    assert!(
        SHIM.contains("_STOP_VALUE_FLAGS = frozenset()"),
        "the shim's stop table is no longer empty, and this one is"
    );
}

/// The flag names in one of the shim's tables, read out of its source.
///
/// Reading text rather than running Python is the point: this side compares its
/// own live constants against the other side's source, and the pytest driver does
/// the mirror image, so neither test can be fooled by misparsing the language it
/// is written in.
fn shim_table(name: &str) -> Vec<&'static str> {
    let opened = format!("{name} = {{");
    let start = SHIM
        .find(&opened)
        .unwrap_or_else(|| panic!("the shim no longer defines {name}"))
        + opened.len();
    // No flag name carries a brace, so the first one closes the set — which is
    // what lets one reader handle the tables written on one line and the tables
    // written over thirty.
    let length = SHIM[start..]
        .find('}')
        .unwrap_or_else(|| panic!("{name} has no closing brace"));
    SHIM[start..start + length]
        .split('"')
        .skip(1)
        .step_by(2)
        .collect()
}
