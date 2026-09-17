//! The completion script's hand-written tables, diffed against the tables that
//! define the grammar.
//!
//! `devlaunch-core/completions/dl.bash` is a bash script with three tables copied
//! into it by hand: the flags `dl` accepts as a first argument, the workspace verbs,
//! and — for the same function serving `aid` — one `--flag` per coding agent. The
//! originals live in three different crates (`dl`'s argument grammar, `aid`'s
//! rewrite table) with nothing linking them, which is how the script came to be
//! missing five flags that had been shipping for releases: a flag added to the
//! grammar completes nothing, and nothing fails. A flag *retired* from the grammar
//! is the same drift the other way, and this catches that too — two spellings the
//! script still offered went hidden while this was in review, and it is what said
//! so.
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
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", relative]
        .iter()
        .collect();
    fs::read_to_string(&path).unwrap_or_else(|why| panic!("{} is readable: {why}", path.display()))
}

fn completion_script() -> String {
    source("devlaunch-core/completions/dl.bash")
}

fn argument_grammar() -> String {
    source("dl/src/cli.rs")
}

fn aid_rewrite() -> String {
    source("aid/src/rewrite.rs")
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

/// What the script offers after a workspace: the verbs, and the flags that ride
/// beside one.
///
/// Two sets rather than one, because they are checked against different originals —
/// the reserved-verb table and the flag table — and the bare `--` that ends the list
/// is neither.
fn dl_workspace_words(script: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let words = assigned(script, "local ws_cmds=");
    assert!(
        words.contains("--"),
        "the workspace list offers `--`, which is how a command is run inside one"
    );
    let (flags, verbs): (BTreeSet<String>, BTreeSet<String>) = words
        .into_iter()
        .filter(|word| word != "--")
        .partition(|word| word.starts_with('-'));
    (verbs, flags)
}

// --- reading the argument grammar's flag table ------------------------------

/// One flag of the `dl` command line, as the grammar declares it.
#[derive(Debug)]
struct Flag {
    /// The `--spelling`, from `long = "..."` or from the field name.
    long: String,
    /// `hide = true`: not in `--help`, so not offered by a completion either.
    hidden: bool,
    /// A value follows this flag as a separate word, which the script has to know
    /// so it can complete the value instead of another flag.
    takes_value: bool,
    /// `group = "what"`: the flag *is* the command, so no workspace follows it.
    /// The group exists because its members are mutually exclusive for exactly
    /// that reason, which is what makes it the original this fact is copied from.
    names_a_command: bool,
    requires_herdr_env: bool,
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
        let (name, ty) = line
            .split_once(':')
            .expect("an argument attribute is followed by its field");
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
            takes_value: ty.trim().trim_end_matches(',') != "bool",
            names_a_command: parts.contains(&"group = \"what\""),
            requires_herdr_env: parts.contains(&"requires = \"herdr_env\""),
        });
    }
    assert!(
        flags.len() > 5,
        "the grammar's flag table was parsed, not merely searched: {flags:?}"
    );
    flags
}

/// The spellings of a `const NAME: [(&str, _); N]` word table in the grammar.
///
/// The declared `N` is checked against the number of entries found, so a table this
/// walks with the wrong shape fails loudly rather than comparing against a short
/// list it happened to scrape.
fn grammar_words(grammar: &str, name: &str) -> BTreeSet<String> {
    let declaration = grammar
        .split_once(&format!("const {name}: [(&str, "))
        .unwrap_or_else(|| panic!("the grammar declares a {name} word table"))
        .1;
    let (shape, entries) = declaration
        .split_once("] = ")
        .unwrap_or_else(|| panic!("{name}'s table has a declared length"));
    let declared: usize = shape
        .split_once("; ")
        .and_then(|(_, count)| count.trim().parse().ok())
        .unwrap_or_else(|| panic!("{name} declares how many entries it has, got {shape:?}"));
    let body = entries
        .split_once("];")
        .unwrap_or_else(|| panic!("{name}'s table ends"))
        .0;

    let words: BTreeSet<String> = body
        .match_indices("(\"")
        .filter_map(|(at, _)| {
            let rest = &body[at + 2..];
            rest.split_once('"').map(|(word, _)| word.to_owned())
        })
        .collect();
    assert_eq!(
        words.len(),
        declared,
        "{name} has {declared} entries; {words:?} was read out of it"
    );
    words
}

// --- reading aid's rewrite tables -------------------------------------------

/// The entries of a `const NAME: &[...] = &[ ... ];` declaration.
fn slice_body<'a>(rust: &'a str, name: &str) -> &'a str {
    let after = rust
        .split_once(&format!("const {name}: &["))
        .unwrap_or_else(|| panic!("aid's rewrite declares {name}"))
        .1;
    after
        .split_once("= &[")
        .unwrap_or_else(|| panic!("{name} is initialised from a slice literal"))
        .1
        .split_once("];")
        .unwrap_or_else(|| panic!("{name}'s declaration ends"))
        .0
}

/// The agents aid can start, from its agent table.
///
/// Matched line by line rather than by scanning for string literals, because the
/// table's entries contain string literals of their own — the words each agent is
/// started with, and the environment pairs. A name is the only entry written as a
/// bare quoted word on a line of its own.
fn aid_agents(rewrite: &str) -> BTreeSet<String> {
    let body = slice_body(rewrite, "AGENTS");
    let names: BTreeSet<String> = body
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix('"')
                .and_then(|rest| rest.strip_suffix("\","))
                .filter(|word| !word.contains('"'))
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(
        names.len(),
        body.matches("Agent {").count(),
        "one name was read per agent in the table, got {names:?}"
    );
    names
}

/// A `const NAME: &[&str] = &["a", "b"];` list of flag spellings.
///
/// An entry may be another `const` rather than a literal — `REMOTE_CONTROL_FLAGS`
/// names `REMOTE_CONTROL_FLAG` so the flag and the list of its spellings cannot
/// disagree — so a bare identifier is resolved to the string that `const` is
/// declared as. Every entry has to resolve to exactly one flag, asserted against
/// the number of entries actually written: reading one of two spellings and
/// comparing the short list against the script is the silent pass this whole file
/// exists to prevent, and it is what happened before the count went in.
fn aid_flag_list(rewrite: &str, name: &str) -> BTreeSet<String> {
    let body = slice_body(rewrite, name);
    let entries: Vec<&str> = body
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let flags: BTreeSet<String> = entries
        .iter()
        .map(
            |part| match part.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                Some(literal) => literal.to_owned(),
                // A named const: `const THAT_NAME: &str = "--flag";`.
                None => named_str_const(rewrite, part),
            },
        )
        .collect();
    assert_eq!(
        flags.len(),
        entries.len(),
        "one flag was read per entry of {name}; {entries:?} became {flags:?}"
    );
    flags
}

/// The string a `const NAME: &str = "...";` in aid's rewrite is declared as.
fn named_str_const(rewrite: &str, name: &str) -> String {
    let after = rewrite
        .split_once(&format!("const {name}: &str = \""))
        .unwrap_or_else(|| panic!("aid's rewrite declares {name} as a &str const"))
        .1;
    after
        .split_once('"')
        .unwrap_or_else(|| panic!("{name}'s literal ends"))
        .0
        .to_owned()
}

/// Flags the grammar accepts that the completion deliberately does not offer for a
/// first argument, each with the reason it is left out.
///
/// **Hand-written, and that is the point.** Everything else is derived, so a flag
/// added to the grammar breaks the test below until somebody either completes it or
/// writes it here — which is the decision this guard exists to force. The common
/// thread is position: each needs something already on the line, so none is ever
/// the first word.
///
/// A leading `--` no longer ends completion by itself — the script tells a flag a
/// spec may follow from one that ends the line, which is what
/// [`the_flags_a_spec_may_follow_are_the_launch_modifiers_the_grammar_leaves_over`]
/// pins — so a launch modifier reaches this table now (`dl --rm --<tab>` offers
/// the other modifiers). These five still do not, and the reason is not one
/// reason: `--json` and `--size` carry `requires = "ls"` and are a clap error
/// without it; `--yes` and `--force-worktrees` are refused by name as meaningless
/// for a workspace command; and a *leading* `--force` is not a modifier at all but
/// the workspace slot itself — `force_placement` reads `--force` with no word
/// before it as `ForcePlace::WorkspaceSlot`, so `dl --force my-ws` answers
/// "Unknown workspace '--force'". All four refusals are what
/// `test_only_a_flag_a_spec_can_follow_is_completed_past` quotes.
const NOT_OFFERED_FIRST: [(&str, &str); 5] = [
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
    (
        "--force-worktrees",
        "modifies --prune alone, which has already been typed by then",
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

#[test]
fn the_workspace_verbs_offered_are_the_grammars_reserved_verbs() {
    let script = completion_script();
    let grammar = argument_grammar();

    let (offered, _) = dl_workspace_words(&script);

    assert_eq!(
        offered,
        grammar_words(&grammar, "VERBS"),
        "the completion script's workspace verbs have drifted from the grammar"
    );
}

#[test]
fn a_retired_verb_is_never_offered() {
    // A word dropped from the verb table is still recognised by name and refused, so
    // completing it would hand somebody a line the CLI answers with "that is not a
    // verb any more". The test above would catch a retired word left in the list
    // today; this one keeps catching it if the two tables are ever edited together.
    //
    // Only the bare word: `prune` is retired as a verb while `--prune` is a current
    // global flag meaning something else entirely, which is exactly why the word was
    // retired, and offering the flag is right.
    let script = completion_script();
    let grammar = argument_grammar();

    let (verbs, _) = dl_workspace_words(&script);
    let retired = grammar_words(&grammar, "RETIRED");

    assert!(!retired.is_empty(), "there is a retired word to check for");
    for word in &retired {
        assert!(
            !verbs.contains(word),
            "{word} is offered as a verb but the grammar retired it"
        );
    }
}

#[test]
fn every_flag_offered_beside_a_workspace_is_a_flag_the_grammar_accepts() {
    // The other half of the diff: the script may not invent a flag either. `--rm` is
    // offered here as well as in first position because riding beside a workspace is
    // what it is for — `dl <ws> --rm` deletes the workspace when the session ends.
    let script = completion_script();
    let grammar = argument_grammar();

    let (_, offered) = dl_workspace_words(&script);
    let accepted: BTreeSet<String> = grammar_flags(&grammar)
        .iter()
        .map(|flag| flag.long.clone())
        .collect();

    assert!(!offered.is_empty(), "the list offers at least one flag");
    for flag in &offered {
        assert!(
            accepted.contains(flag),
            "{flag} is offered beside a workspace but the grammar does not accept it"
        );
    }
}

#[test]
fn the_flags_a_value_follows_are_the_grammars_value_taking_flags() {
    // Three copies of this one fact, so all three are compared: the script switches
    // to completing a value after these, aid has to tell such a value from the
    // workspace spec, and the grammar is where the flag's value is declared. A
    // second value-taking flag added to the grammar and missed here would complete
    // as though a flag came next.
    let script = completion_script();
    let grammar = argument_grammar();
    let rewrite = aid_rewrite();

    let expected: BTreeSet<String> = grammar_flags(&grammar)
        .iter()
        .filter(|flag| flag.takes_value && !flag.hidden)
        .map(|flag| flag.long.clone())
        .collect();

    assert_eq!(assigned(&script, "local value_opts="), expected);
    let launch_values: BTreeSet<String> = grammar_flags(&grammar)
        .iter()
        .filter(|flag| flag.takes_value && !flag.hidden && !flag.requires_herdr_env)
        .map(|flag| flag.long.clone())
        .collect();
    assert_eq!(aid_flag_list(&rewrite, "DL_VALUE_OPTIONS"), launch_values);
    assert!(
        expected.is_subset(&dl_first_argument_flags(&script)),
        "a flag whose value is completed is a flag that can be reached"
    );
}

#[test]
fn the_flags_a_spec_may_follow_are_the_launch_modifiers_the_grammar_leaves_over() {
    // The script has to tell a flag a workspace spec still follows from one that
    // ends the line, because that is what decides whether to complete a spec at
    // all: `dl --devcontainer robot` has not started and `dl --ls` is finished.
    //
    // Listed in the direction of the exceptions, and derived by subtraction, for
    // the reason the script's own comment gives: the flags nobody thinks to list
    // are all on the ending side, so a hand-written list of *those* is the one
    // that silently drifts. Three subtrahends, each already pinned by a test in
    // this file or forced by `NOT_OFFERED_FIRST`:
    //
    //   every flag  -  clap's `what` group  -  the hidden ones  -  NOT_OFFERED_FIRST
    //
    // `what` because each of its members is the whole command; hidden because a
    // spelling `--help` does not show is one only a retired build still answers
    // (`--stop`, `--autorm`) or an internal re-entry (`--repos`,
    // `--completion-data`, `--update-cache`), and none of those takes a
    // workspace; `NOT_OFFERED_FIRST` because each of those five modifies a
    // *command* that is already on the line, which is the very thing whose
    // presence ends it.
    //
    // What survives is the launch modifiers, and the subtraction is what makes
    // that a derivation rather than a second opinion.
    let script = completion_script();
    let grammar = argument_grammar();

    let withheld: BTreeSet<&str> = NOT_OFFERED_FIRST.iter().map(|(flag, _)| *flag).collect();
    let expected: BTreeSet<String> = grammar_flags(&grammar)
        .iter()
        .filter(|flag| {
            !flag.names_a_command
                && !flag.hidden
                && !flag.requires_herdr_env
                && !withheld.contains(flag.long.as_str())
        })
        .map(|flag| flag.long.clone())
        .collect();

    assert!(
        !expected.is_empty(),
        "the subtraction leaves something: a spec follows at least one flag"
    );
    assert_eq!(
        assigned(&script, "local spec_follows="),
        expected,
        "the completion script's launch modifiers have drifted from the grammar"
    );
    assert!(
        expected.is_subset(&dl_first_argument_flags(&script)),
        "a flag a spec follows is a flag that can be tabbed to in the first place"
    );
}

#[test]
fn every_flag_whose_value_is_completed_is_one_a_spec_may_follow() {
    // The scan steps over a value option *and its value* and then keeps looking
    // for the spec, so a value option missing from `spec_follows` would eat its
    // value and end the line in the same breath -- `dl --devcontainer robot
    // owner/repo` would complete nothing, which is half of the defect this whole
    // change fixes. Checked for both binaries, because aid's table is its own.
    let script = completion_script();

    let management_values: BTreeSet<String> = grammar_flags(&argument_grammar())
        .iter()
        .filter(|flag| flag.takes_value && flag.requires_herdr_env)
        .map(|flag| flag.long.clone())
        .collect();
    let values: BTreeSet<String> = assigned(&script, "local value_opts=")
        .difference(&management_values)
        .cloned()
        .collect();
    for table in ["local spec_follows=", "spec_follows="] {
        let follows = assigned(&script, table);
        for flag in &values {
            assert!(
                follows.contains(flag),
                "{flag} takes a value that {table} does not let a spec follow"
            );
        }
    }
}

#[test]
fn a_flag_offered_beside_a_workspace_is_one_a_spec_could_have_followed() {
    // `ws_cmds` offers `--rm` *after* a spec and `spec_follows` lets one come
    // after it, and both readings have to hold of the same word: `dl --rm <ws>`
    // and `dl <ws> --rm` are the same request written either way round, so a
    // flag in one table and not the other would answer them differently.
    let script = completion_script();

    let follows = assigned(&script, "local spec_follows=");
    let (_, beside) = dl_workspace_words(&script);

    assert!(!beside.is_empty(), "the list offers at least one flag");
    for flag in &beside {
        assert!(
            follows.contains(flag),
            "{flag} is offered beside a workspace but no spec may follow it"
        );
    }
}

/// Flags `aid` offers for a first argument beyond one per agent, and where each
/// comes from.
///
/// Hand-written for the same reason as [`NOT_OFFERED_FIRST`]: aid's grammar is not a
/// clap grammar and could not be one (an unknown leading flag is passed through to
/// dl, and everything after the workspace is prompt), so its help text is the
/// interface and there is no table to derive these two from. The agent flags, which
/// are a table, are derived.
///
/// aid's `--rm`, `--stop`, `--autorm` and `--force` are absent on purpose: they are
/// *suffix* flags, peeled off the end of a line, and the script offers nothing after
/// an aid workspace because everything there is prompt.
const AID_FLAGS_BESIDE_THE_AGENTS: [(&str, &str); 3] = [
    ("--help", "aid's own, checked before anything is rewritten"),
    ("-h", "the same"),
    ("--version", "aid's own, likewise"),
];

#[test]
fn aid_offers_one_flag_per_agent_it_can_start() {
    let script = completion_script();
    let rewrite = aid_rewrite();

    let mut expected: BTreeSet<String> = aid_agents(&rewrite)
        .iter()
        .map(|agent| format!("--{agent}"))
        .collect();
    expected.extend(aid_flag_list(&rewrite, "DL_VALUE_OPTIONS"));
    expected.extend(
        AID_FLAGS_BESIDE_THE_AGENTS
            .iter()
            .map(|(flag, _)| (*flag).to_owned()),
    );

    assert_eq!(
        assigned(&script, "global_opts="),
        expected,
        "the completion script's aid flags have drifted from aid's agent table"
    );
}

#[test]
fn the_aid_flags_a_spec_may_follow_are_the_ones_parse_aid_args_reads_past() {
    // aid's half of `the_flags_a_spec_may_follow_are_the_launch_modifiers_...`, and
    // derived the same way even though aid's grammar is not a clap one: these are
    // exactly the three tables `parse_aid_args` reads and then keeps looking for the
    // spec -- an agent flag, either polarity of the remote-control flag, or a dl
    // value option.
    //
    // The three flags aid answers itself are absent, and so is an unknown flag: aid
    // passes one through to dl as a boolean option, so on `aid --unknown-taking-a-
    // value foo owner/repo` it calls `foo` the spec, and no completion is honest
    // about a slot aid itself cannot place.
    let script = completion_script();
    let rewrite = aid_rewrite();

    let mut expected: BTreeSet<String> = aid_agents(&rewrite)
        .iter()
        .map(|agent| format!("--{agent}"))
        .collect();
    for table in [
        "REMOTE_CONTROL_FLAGS",
        "NO_REMOTE_CONTROL_FLAGS",
        "DL_VALUE_OPTIONS",
    ] {
        expected.extend(aid_flag_list(&rewrite, table));
    }

    assert_eq!(
        assigned(&script, "spec_follows="),
        expected,
        "aid's launch modifiers have drifted from what `parse_aid_args` reads past"
    );
    for (flag, _) in AID_FLAGS_BESIDE_THE_AGENTS {
        assert!(
            !expected.contains(flag),
            "{flag} is one aid answers itself, so no spec follows it"
        );
    }
}
