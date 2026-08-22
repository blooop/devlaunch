//! The completion script's hand-written tables, diffed against the tables that
//! define the grammar.
//!
//! `devlaunch-core/completions/dl.bash` is a bash script with three tables copied
//! into it by hand: the flags `dl` accepts as a first argument, the workspace verbs,
//! and — for the same function serving `aid` — one `--flag` per coding agent. The
//! originals live in three different crates (`dl`'s argument grammar, `aid`'s
//! rewrite table) with nothing linking them, which is how the script came to be
//! missing seven flags that had been shipping for releases: a flag added to the
//! grammar completes nothing, and nothing fails.
//!
//! This is the diff that fails instead. It is a **tactical stop-loss**, not a
//! design: the real fix is a completion script generated from the grammar, which is
//! a separate question (the "Drift-proofing rapid development" map). Until that
//! lands, the hand-written copy is safe to keep because drifting from the original
//! breaks a test.
//!
//! # Why the tables are read as text
//!
//! Every table this compares against is private to its crate — `dl`'s `Cli` and its
//! verb list are `pub(crate)` or narrower, `aid`'s agent table is module-private —
//! and widening a type's visibility so a test can see it is a worse trade than
//! reading the file the table is written in. So the extraction below parses source
//! text, and every extractor asserts it found something: a parser that silently
//! matched nothing would turn this whole file into a test that passes by finding no
//! facts to compare.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// The completion payload, `dl`'s argument grammar, and `aid`'s rewrite table,
/// resolved from this crate's manifest directory so the test does not care where it
/// was run from.
fn source(relative: &str) -> String {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", relative].iter().collect();
    fs::read_to_string(&path).unwrap_or_else(|why| panic!("{} is readable: {why}", path.display()))
}

fn completion_script() -> String {
    source("devlaunch-core/completions/dl.bash")
}

fn argument_grammar() -> String {
    source("dl/src/cli.rs")
}

// --- reading the bash script's tables ---------------------------------------

/// The words of one `NAME="a b c"` assignment in the script, found by the exact
/// text the line starts with.
///
/// The prefix rather than just the variable name, because `global_opts` is assigned
/// twice — once as the `local` default for `dl` and once, bare, in the branch that
/// serves `aid` — and the two tables are compared against different originals.
/// Exactly one line may match, so a second copy of an assignment fails here rather
/// than being silently ignored.
fn assigned(script: &str, prefix: &str) -> BTreeSet<String> {
    let found: Vec<&str> = script
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix(prefix))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "exactly one line of the completion script starts with {prefix:?}, found {}",
        found.len()
    );
    let value = found[0]
        .trim()
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| {
            panic!(
                "{prefix:?} assigns a double-quoted list, got {:?}",
                found[0]
            )
        });
    let words: BTreeSet<String> = value.split_whitespace().map(str::to_owned).collect();
    assert!(!words.is_empty(), "{prefix:?} assigns a non-empty list");
    words
}

/// The flags `dl` offers for a first argument.
fn dl_first_argument_flags(script: &str) -> BTreeSet<String> {
    assigned(script, "local global_opts=")
}

// --- reading the argument grammar's flag table ------------------------------

/// One flag of the `dl` command line, as the grammar declares it.
#[derive(Debug)]
struct Flag {
    /// The `--spelling`, from `long = "..."` or from the field name.
    long: String,
    /// `hide = true`: not in `--help`, so not offered by a completion either.
    hidden: bool,
}

/// Every flag the `Cli` struct declares, in declaration order.
///
/// Fields with no `long` are the positional words, which are not flags.
fn grammar_flags(grammar: &str) -> Vec<Flag> {
    let body = grammar
        .split_once("pub(crate) struct Cli {")
        .expect("the grammar declares a Cli struct")
        .1
        .split_once("\n}")
        .expect("the Cli struct's body ends")
        .0;

    let mut flags = Vec::new();
    let mut pending: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#[arg(") {
            let inner = rest
                .strip_suffix(")]")
                .expect("an #[arg(...)] attribute is written on one line");
            pending = Some(inner.to_owned());
            continue;
        }
        let Some(attribute) = pending.take() else {
            continue;
        };
        let name = line
            .split_once(':')
            .expect("an argument attribute is followed by its field")
            .0;
        let parts: Vec<&str> = attribute.split(',').map(str::trim).collect();
        let long = parts.iter().find_map(|part| {
            if *part == "long" {
                Some(name.replace('_', "-"))
            } else {
                part.strip_prefix("long = \"")
                    .and_then(|rest| rest.strip_suffix('"'))
                    .map(str::to_owned)
            }
        });
        let Some(long) = long else { continue };
        flags.push(Flag {
            long: format!("--{long}"),
            hidden: parts.contains(&"hide = true"),
        });
    }
    assert!(
        flags.len() > 5,
        "the grammar's flag table was parsed, not merely searched: {flags:?}"
    );
    flags
}

/// Flags the grammar accepts that the completion deliberately does not offer for a
/// first argument, each with the reason it is left out.
///
/// **Hand-written, and that is the point.** Everything else is derived, so a flag
/// added to the grammar breaks the test below until somebody either completes it or
/// writes it here — which is the decision this guard exists to force. The common
/// thread is position: the completion offers this table where a *command* goes, and
/// none of these is one. They modify a line that already named one, and the script
/// offers nothing in that position at all (a first word starting with `--` ends
/// completion) — a gap worth closing, but a different change than this.
const NOT_OFFERED_FIRST: [(&str, &str); 4] = [
    (
        "--json",
        "only with --ls, which has already been typed by then",
    ),
    ("--size", "only with --ls, likewise"),
    (
        "--yes",
        "answers the confirmation of a command already on the line",
    ),
    (
        "--force",
        "modifies rm, --prune or --update-cache, never alone",
    ),
];

#[test]
fn every_user_facing_dl_flag_is_offered_for_a_first_argument() {
    let script = completion_script();
    let grammar = argument_grammar();

    let withheld: BTreeSet<&str> = NOT_OFFERED_FIRST.iter().map(|(flag, _)| *flag).collect();
    let mut expected: BTreeSet<String> = grammar_flags(&grammar)
        .iter()
        .filter(|flag| !flag.hidden && !withheld.contains(flag.long.as_str()))
        .map(|flag| flag.long.clone())
        .collect();
    // clap generates these two; no field declares them.
    expected.insert("--help".to_owned());
    expected.insert("-h".to_owned());

    assert_eq!(
        dl_first_argument_flags(&script),
        expected,
        "the completion script's first-argument flags have drifted from the grammar"
    );
}
