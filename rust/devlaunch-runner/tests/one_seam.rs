//! `ProcessRunner` is the only [`Runner`] outside test code, asserted rather than
//! asked for.
//!
//! The trait's doc used to say it was "implemented once for real processes and
//! once for tests", and by the time anyone read that sentence there were nine
//! impls. Rewriting it as a count would rot the same way, so the doc now states a
//! shape — one production impl, one shared fake, wrappers over those two — and
//! this test holds the half of that shape which is load-bearing. A second
//! implementation doing real work is not a doc that has gone stale: it is a second
//! way for devlaunch to start a process, and the seam has stopped being one.
//!
//! The instrument is still "read the source", because the alternative is a
//! hand-written list of the wrappers, which is the artefact that rotted in the
//! first place. What changed is the unit. A line-at-a-time scan matching the
//! literal text `impl Runner for ` reads three ordinary spellings wrong, and every
//! one of the three was demonstrated against the first version of this file:
//!
//! - `impl<R: Runner> Runner for Timed<R>` does not begin with that text, so a
//!   generic wrapper — the exact shape the trait's doc invites — walked past it.
//! - `impl devlaunch_runner::Runner for X` does not either, which is how the trait
//!   is named from any crate that has not imported it: the likeliest spelling of a
//!   genuinely new second seam.
//! - `#[cfg(test)]` was only read as a gate when `mod tests {` was the very next
//!   line, so one `#[allow(...)]` between them turned every wrapper in that module
//!   into a reported production impl.
//!
//! So the unit is now a token rather than a line: comments and literals are
//! blanked out first — which is also why the doc sentence quoting the grep, and
//! the fixtures below, do not count as implementations — and `#[cfg(test)]` gates
//! a brace-matched region rather than the line beneath it. That last one closes a
//! fourth hole nobody had reached yet: the old scan called *everything* below an
//! inline test module test code, including whatever was written after the module
//! closed.
//!
//! Everything the scan cannot see for itself is [`TEST_ONLY_FILES`], where
//! dropping a file is a visible edit rather than an omission.
//!
//! [`Runner`]: devlaunch_runner::Runner

use std::fs;
use std::path::{Path, PathBuf};

/// Files whose every impl is test code but which do not say so in the file: the
/// module is declared `#[cfg(test)] mod ...` somewhere else. Each one arrived as
/// a failure here rather than as silence, which is the point of the list.
///
/// `agent_worktrees/tests.rs` holds a wrapper that does real work on purpose:
/// the sweep's answers come out of real `git worktree list` output, and it keeps
/// every argv so a test can assert an invocation was *never* made.
const TEST_ONLY_FILES: &[&str] = &[
    "devlaunch-core/src/testing.rs",
    "devlaunch-core/src/flows/agent_worktrees/tests.rs",
];

/// The one implementation that may do real work.
const PRODUCTION: &str = "ProcessRunner";

/// The last segment of the trait's path, whatever the impl calls it by.
const SEAM: &[u8] = b"Runner";

/// An implementation of the seam: what it implements the seam for, the file it
/// was read out of, and whether the scan judged it test code.
struct Found {
    path: String,
    name: String,
    test: bool,
}

// ---------------------------------------------------------------------------
// Blanking out what is not code.
// ---------------------------------------------------------------------------

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    let to = to.min(out.len());
    for byte in &mut out[from..to] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// The source with every comment, string and char literal replaced by spaces.
///
/// Offsets and line breaks survive, so the result can be scanned as if it were the
/// file. Doing this first is what makes the rest of the scan safe to write with
/// byte matching: a brace inside a comment cannot unbalance a `#[cfg(test)]`
/// region, and `"impl Runner for "` written as a string — this file used to hold
/// exactly that constant, and the trait's doc quotes the grep — is not an impl.
fn code_only(text: &str) -> Vec<u8> {
    let src = text.as_bytes();
    let mut out = src.to_vec();
    let mut at = 0;
    while at < src.len() {
        let start = at;
        match src[at] {
            b'/' if src.get(at + 1) == Some(&b'/') => {
                while at < src.len() && src[at] != b'\n' {
                    at += 1;
                }
                blank(&mut out, start, at);
            }
            b'/' if src.get(at + 1) == Some(&b'*') => {
                at += 2;
                let mut depth = 1usize;
                while at < src.len() && depth > 0 {
                    if src[at] == b'/' && src.get(at + 1) == Some(&b'*') {
                        depth += 1;
                        at += 2;
                    } else if src[at] == b'*' && src.get(at + 1) == Some(&b'/') {
                        depth -= 1;
                        at += 2;
                    } else {
                        at += 1;
                    }
                }
                blank(&mut out, start, at);
            }
            // A raw string, `r"..."` or `r#"..."#`, but not the raw identifier
            // `r#type` and not the `r` at the end of somebody's name.
            b'r' if start == 0 || !is_ident_byte(src[start - 1]) => {
                let mut hashes = 0;
                let mut probe = at + 1;
                while src.get(probe) == Some(&b'#') {
                    hashes += 1;
                    probe += 1;
                }
                if src.get(probe) != Some(&b'"') {
                    at += 1;
                    continue;
                }
                at = probe + 1;
                loop {
                    match src.get(at) {
                        None => break,
                        Some(&b'"') => {
                            let closed = (1..=hashes).all(|n| src.get(at + n) == Some(&b'#'));
                            at += 1;
                            if closed {
                                at += hashes;
                                break;
                            }
                        }
                        Some(_) => at += 1,
                    }
                }
                blank(&mut out, start, at);
            }
            b'"' => {
                at += 1;
                while at < src.len() && src[at] != b'"' {
                    at += if src[at] == b'\\' { 2 } else { 1 };
                }
                at = (at + 1).min(src.len());
                blank(&mut out, start, at);
            }
            // `'a'` is a char literal; `'a` is a lifetime. Only the first is
            // blanked, and the difference is whether a closing quote follows the
            // one character.
            b'\'' => {
                let escaped = src.get(at + 1) == Some(&b'\\');
                if escaped || src.get(at + 2) == Some(&b'\'') {
                    at += 1;
                    while at < src.len() && src[at] != b'\'' {
                        at += if src[at] == b'\\' { 2 } else { 1 };
                    }
                    at = (at + 1).min(src.len());
                    blank(&mut out, start, at);
                } else {
                    at += 1;
                }
            }
            _ => at += 1,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Walking blanked source.
// ---------------------------------------------------------------------------

fn skip_space(src: &[u8], mut at: usize) -> usize {
    while at < src.len() && src[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

/// The offset just past the balanced `<...>` opening at `at`.
///
/// `->` is stepped over whole, so a bound like `R: Fn() -> T` does not close the
/// parameter list early.
fn past_angles(src: &[u8], mut at: usize) -> usize {
    let mut depth = 0usize;
    while at < src.len() {
        match src[at] {
            b'-' if src.get(at + 1) == Some(&b'>') => at += 1,
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
        at += 1;
    }
    at
}

/// The offset of the `}` closing the `{` at `at`.
fn closing_brace(src: &[u8], mut at: usize) -> usize {
    let mut depth = 0usize;
    while at < src.len() {
        match src[at] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return at;
                }
            }
            _ => {}
        }
        at += 1;
    }
    src.len()
}

/// The end of the identifier starting at `at`, if one does.
fn ident_end(src: &[u8], at: usize) -> Option<usize> {
    let mut end = at;
    while end < src.len() && is_ident_byte(src[end]) {
        end += 1;
    }
    (end > at).then_some(end)
}

/// A `foo::bar::Baz` path starting at `at`: where it ends, and its last segment.
fn path(src: &[u8], mut at: usize) -> Option<(usize, &[u8])> {
    if src.get(at) == Some(&b':') && src.get(at + 1) == Some(&b':') {
        at += 2;
    }
    let mut end = ident_end(src, at)?;
    let mut last = &src[at..end];
    while src.get(end) == Some(&b':') && src.get(end + 1) == Some(&b':') {
        let from = end + 2;
        end = ident_end(src, from)?;
        last = &src[from..end];
    }
    Some((end, last))
}

/// Whether `word` sits at `at` as a whole token.
fn keyword_at(src: &[u8], at: usize, word: &[u8]) -> bool {
    src.len() >= at + word.len()
        && &src[at..at + word.len()] == word
        && !src.get(at + word.len()).copied().is_some_and(is_ident_byte)
}

/// The `#[...]` attribute opening at `at`, if one does: the offset just past its
/// closing bracket.
fn past_attribute(src: &[u8], at: usize) -> Option<usize> {
    let mut probe = at + 1;
    if src.get(probe) == Some(&b'!') {
        probe += 1;
    }
    if src.get(probe) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    while probe < src.len() {
        match src[probe] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(probe + 1);
                }
            }
            _ => {}
        }
        probe += 1;
    }
    None
}

/// Whether an attribute's text gates on `test`.
///
/// Read off blanked source, so `#[cfg(feature = "test-support")]` cannot match:
/// the only `test` left in a literal is one nobody wrote as a string.
fn gates_on_test(attribute: &[u8]) -> bool {
    let has = |word: &[u8]| {
        (0..attribute.len()).any(|at| {
            keyword_at(attribute, at, word) && (at == 0 || !is_ident_byte(attribute[at - 1]))
        })
    };
    has(b"cfg") && has(b"test")
}

/// The byte ranges this file puts behind `#[cfg(test)]`.
///
/// A gate covers the item it is written on, from the attribute to the closing
/// brace of that item's body — the module, the impl, the function. Attributes
/// between the gate and the item are skipped rather than ending the search, which
/// is the whole of the false positive this replaced: `#[cfg(test)]` followed by
/// `#[allow(...)]` followed by `mod tests {` is still a test module. A gate on a
/// declaration with no body (`#[cfg(test)] mod tests;`) covers nothing here — the
/// file it names is judged on its own, or listed in [`TEST_ONLY_FILES`].
fn test_gated(src: &[u8]) -> Vec<(usize, usize)> {
    let mut gated = Vec::new();
    let mut at = 0;
    while at < src.len() {
        if src[at] != b'#' {
            at += 1;
            continue;
        }
        let Some(mut after) = past_attribute(src, at) else {
            at += 1;
            continue;
        };
        if gates_on_test(&src[at..after]) {
            loop {
                let next = skip_space(src, after);
                match src.get(next) {
                    Some(&b'#') => match past_attribute(src, next) {
                        Some(past) => after = past,
                        None => break,
                    },
                    _ => break,
                }
            }
            let mut item = skip_space(src, after);
            while item < src.len() && src[item] != b'{' && src[item] != b';' {
                item += 1;
            }
            if src.get(item) == Some(&b'{') {
                gated.push((at, closing_brace(src, item)));
            }
        }
        at = after;
    }
    gated
}

/// Every `impl ... Runner for Type` in blanked source: where it starts, and the
/// name of the type it is written for.
///
/// The trait may be reached by any path (`Runner`, `devlaunch_runner::Runner`,
/// `crate::Runner`) and the impl may carry any generics, which is the pair of
/// bypasses this replaced. An inherent `impl Type` has no `for` and is skipped.
fn implementations_in(src: &[u8]) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < src.len() {
        if !keyword_at(src, at, b"impl") || (at > 0 && is_ident_byte(src[at - 1])) {
            at += 1;
            continue;
        }
        let opens = at;
        at += 4;
        let mut probe = skip_space(src, at);
        if src.get(probe) == Some(&b'<') {
            probe = skip_space(src, past_angles(src, probe));
        }
        let Some((after_trait, segment)) = path(src, probe) else {
            continue;
        };
        if segment != SEAM {
            continue;
        }
        let mut probe = skip_space(src, after_trait);
        if src.get(probe) == Some(&b'<') {
            probe = skip_space(src, past_angles(src, probe));
        }
        if !keyword_at(src, probe, b"for") {
            continue;
        }
        probe = skip_space(src, probe + 3);
        while src.get(probe) == Some(&b'&') || src.get(probe) == Some(&b'\'') {
            probe = skip_space(src, probe + 1);
            if keyword_at(src, probe, b"mut") {
                probe = skip_space(src, probe + 3);
            }
            while probe < src.len() && is_ident_byte(src[probe]) {
                probe += 1;
            }
            probe = skip_space(src, probe);
        }
        if let Some((_, name)) = path(src, probe) {
            found.push((opens, String::from_utf8_lossy(name).into_owned()));
        }
    }
    found
}

/// Every implementation of the seam in one file, classified.
///
/// Three ways to be test code: living in a test-support crate, living under a
/// `tests/` directory or [`TEST_ONLY_FILES`], or sitting inside a `#[cfg(test)]`
/// item — which is where every wrapper a unit test defines sits.
fn scan(relative: &str, text: &str) -> Vec<Found> {
    let src = code_only(text);
    let gated = test_gated(&src);
    // Read from the workspace-relative path, not the absolute one: whatever
    // directories this checkout happens to live under are not evidence about the
    // code.
    let test_tree = relative
        .split('/')
        .any(|part| part.ends_with("-test-support") || part == "tests")
        || TEST_ONLY_FILES.contains(&relative);
    implementations_in(&src)
        .into_iter()
        .map(|(at, name)| Found {
            path: relative.to_owned(),
            name,
            test: test_tree || gated.iter().any(|&(from, to)| (from..=to).contains(&at)),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The workspace.
// ---------------------------------------------------------------------------

/// The cargo workspace root: this crate's directory, one level up.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the cargo workspace above this crate")
        .to_path_buf()
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|why| panic!("reading {}: {why}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            // `target` by name, and by cargo's own marker, so a build directory
            // that `CARGO_TARGET_DIR` moved or renamed does not feed generated
            // sources into the scan.
            if path.file_name().is_some_and(|name| name == "target")
                || path.join("CACHEDIR.TAG").is_file()
            {
                continue;
            }
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|kind| kind == "rs") {
            found.push(path);
        }
    }
}

/// Every implementation of the seam in the workspace.
fn implementations() -> Vec<Found> {
    let root = workspace();
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    sources.sort();

    let mut found = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|why| panic!("reading {}: {why}", path.display()));
        let relative = path
            .strip_prefix(&root)
            .expect("a path under the workspace")
            .to_string_lossy()
            .into_owned();
        found.extend(scan(&relative, &text));
    }
    found
}

fn describe(found: &[Found]) -> Vec<String> {
    found
        .iter()
        .map(|one| {
            let kind = if one.test { "test" } else { "production" };
            format!("{} ({}, {kind})", one.name, one.path)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The scanner, against the spellings that broke the line-at-a-time version.
// ---------------------------------------------------------------------------

/// What the scanner made of one file's worth of source, as `name/production` or
/// `name/test`, so a case can be written as one line.
fn verdicts(relative: &str, text: &str) -> Vec<String> {
    scan(relative, text)
        .into_iter()
        .map(|one| {
            format!(
                "{}/{}",
                one.name,
                if one.test { "test" } else { "production" }
            )
        })
        .collect()
}

#[test]
fn a_generic_wrapper_is_an_implementation() {
    // The shape the trait's doc invites, and the one the line scan missed: the
    // text does not begin `impl Runner for `.
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "pub struct Sneaky2<'a>(&'a str);\nimpl<'a> Runner for Sneaky2<'a> {}\n",
        ),
        ["Sneaky2/production"]
    );
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "impl<R: Runner + Send> Runner for Timed<R> {}\n",
        ),
        ["Timed/production"]
    );
}

#[test]
fn a_trait_named_by_its_path_is_the_same_trait() {
    // The spelling any crate that has not imported the trait would use, which is
    // the likeliest form of a genuinely new second seam.
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "impl devlaunch_runner::Runner for Sneaky3 {}\n",
        ),
        ["Sneaky3/production"]
    );
    assert_eq!(
        verdicts("dl/src/probe.rs", "impl crate::Runner for Sneaky4 {}\n"),
        ["Sneaky4/production"]
    );
}

#[test]
fn an_attribute_between_the_gate_and_the_module_is_still_the_gate() {
    // The false positive: one `#[allow(...)]` used to turn every wrapper in the
    // module into a reported production impl, accusing its author of adding a
    // second seam.
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "#[cfg(test)]\n#[allow(clippy::too_many_lines)]\nmod tests {\n    struct LocalFake;\n    impl Runner for LocalFake {}\n}\n",
        ),
        ["LocalFake/test"]
    );
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "#[cfg(any(test, feature = \"x\"))]\nmod tests {\n    impl Runner for LocalFake {}\n}\n",
        ),
        ["LocalFake/test"]
    );
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "#[cfg(test)]\nimpl Runner for LocalFake {}\n",
        ),
        ["LocalFake/test"]
    );
}

#[test]
fn the_gate_ends_where_the_module_does() {
    // The hole nobody had reached: everything below an inline test module used to
    // read as test code, so the way past the guard was to write underneath it.
    assert_eq!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "#[cfg(test)]\nmod tests {\n    impl Runner for LocalFake {}\n}\nimpl Runner for Afterwards {}\n",
        ),
        ["LocalFake/test", "Afterwards/production"]
    );
}

#[test]
fn a_gate_on_a_declaration_swallows_nothing() {
    // `devlaunch-runner/src/lib.rs` really is written this way, with the
    // production impl hundreds of lines below the declaration.
    assert_eq!(
        verdicts(
            "devlaunch-runner/src/lib.rs",
            "#[cfg(test)]\nmod tests;\n\nimpl Runner for ProcessRunner {}\n",
        ),
        ["ProcessRunner/production"]
    );
}

#[test]
fn only_code_counts() {
    // Both of these are in this tree: the trait's doc names the grep, and a test
    // fixture spells an impl out as a string. Neither is an implementation, and a
    // scan over raw text calls both one.
    assert!(
        verdicts(
            "devlaunch-runner/src/lib.rs",
            "/// `grep -rn \"impl Runner for\"` is the enumeration.\npub trait Runner {}\n",
        )
        .is_empty()
    );
    assert!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "const OPENER: &str = \"impl Runner for \";\n",
        )
        .is_empty()
    );
    assert!(
        verdicts(
            "devlaunch-core/src/probe.rs",
            "/* impl Runner for Commented {} */\nimpl Something for Real {}\n",
        )
        .is_empty()
    );
}

#[test]
fn an_inherent_impl_is_not_an_implementation_of_the_seam() {
    assert!(
        verdicts(
            "devlaunch-runner/src/lib.rs",
            "impl Runner {}\nimpl ProcessRunner {}\nimpl Debug for Runner {}\n",
        )
        .is_empty()
    );
}

// ---------------------------------------------------------------------------
// The workspace, against the shape the trait's doc states.
// ---------------------------------------------------------------------------

#[test]
fn the_scan_finds_the_implementations_it_is_meant_to_judge() {
    let found = implementations();
    assert!(
        found
            .iter()
            .any(|one| one.path.contains("devlaunch-runner") && one.name == PRODUCTION),
        "the scan did not find {PRODUCTION}, so it is not reading this workspace: {:#?}",
        describe(&found)
    );
    assert!(
        found.iter().filter(|one| one.test).count() >= 2,
        "the scan found no test implementations, which no version of this tree has \
         been true of: {:#?}",
        describe(&found)
    );
}

#[test]
fn only_process_runner_does_real_work() {
    let production: Vec<_> = implementations()
        .into_iter()
        .filter(|one| !one.test)
        .map(|one| format!("{} ({})", one.name, one.path))
        .collect();
    assert_eq!(
        production.len(),
        1,
        "Runner is the one seam onto the OS, and {PRODUCTION} is meant to be the \
         only implementation of it outside test code. Found: {production:#?}. A \
         second real implementation is a second way to start a process; if that is \
         deliberate, the trait's doc in src/lib.rs says otherwise and needs the \
         edit first."
    );
    assert!(
        production[0].starts_with(PRODUCTION),
        "the one production Runner is no longer {PRODUCTION}: {production:#?}"
    );
}
