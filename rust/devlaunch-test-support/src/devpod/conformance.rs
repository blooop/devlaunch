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
//! Rows assert exit code and workspace state, and nothing about stdout: real
//! devpod answers a missing-and-ignored delete with a timestamped log line no
//! fake spells, so pinning text would pin a fake's invention. The output shapes
//! that matter are pinned against recorded fixtures in each fake's own tests.

use super::*;
use devlaunch_runner::Exit;
use serde::Deserialize;

/// The corpus, read at compile time from the directory both suites share.
const CORPUS: &str = include_str!("../../../../test/fixtures/devpod/conformance.json");

#[derive(Debug, Deserialize)]
struct Corpus {
    rows: Vec<Row>,
}

/// One row: seeded state, argv, expected exit, expected workspaces afterwards.
#[derive(Debug, Deserialize)]
struct Row {
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
    for row in corpus().rows {
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
}

#[test]
fn the_corpus_carries_the_rows_the_decision_named() {
    // The minimum set #309 committed to: the drift that motivated the corpus,
    // every `up` value flag production sends that both fakes mis-parsed, and
    // ssh's `--workdir`. A row silently dropped is a guard silently removed.
    let rows = corpus().rows;
    let all: Vec<&[String]> = rows.iter().map(|row| row.argv.as_slice()).collect();

    let mentions = |flag: &str| all.iter().any(|argv| argv.iter().any(|arg| arg == flag));
    for flag in [
        "--ignore-not-found",
        "--init-env",
        "--dotfiles-script",
        "--mount",
        "--dotfiles-script-env",
        "--workspace-env-file",
        "--workdir",
    ] {
        assert!(mentions(flag), "no corpus row exercises {flag}");
    }

    // And each of those value flags is exercised ahead of the positional, which
    // is the shape that tells a value flag from a bare one.
    for (verb, flag) in [
        ("up", "--init-env"),
        ("up", "--dotfiles-script"),
        ("up", "--mount"),
        ("up", "--dotfiles-script-env"),
        ("up", "--workspace-env-file"),
        ("ssh", "--workdir"),
    ] {
        assert!(
            all.iter()
                .any(|argv| argv.first().map(String::as_str) == Some(verb)
                    && argv.get(1).map(String::as_str) == Some(flag)),
            "no corpus row puts {flag} ahead of {verb}'s positional"
        );
    }
}

#[test]
fn every_row_says_how_it_was_verified() {
    // The corpus is only worth its keep if a row's provenance travels with it: a
    // behaviour measured against the real binary and one inherited from the two
    // fakes agreeing are different claims, and the drift got in by collapsing
    // them. So every row has to say which it is, in words.
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
    }
}
