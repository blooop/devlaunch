//! One shell word, spelled the way Python spells it.
//!
//! Everything devlaunch sends to a remote shell is composed out of this: the
//! `bash -lc <payload>` a launch runs, the `cd <workdir> && …` an OpenSSH session
//! prefixes, the generated provisioning scripts, and the `dl` command line `aid`
//! builds. They all have to agree, and they all have to agree with `shlex.quote` —
//! because the payload travels in **argv**, and argv is what the parity harness
//! compares.
//!
//! Hand-written rather than delegated to the `shlex` crate, and the difference is
//! bytes rather than taste. The crate quotes a word Python leaves bare (`a@b%c`),
//! and for a word containing a single quote it switches between double- and
//! single-quoted segments (`"a'b"'$c'`) where Python writes one single-quoted word
//! with `'"'"'` for each embedded quote. Both are the same word to a POSIX shell,
//! and only one of them is the same bytes. The crate is still what reads a payload
//! *back*: the tests split a composed `--command` with `shlex::split` and compare
//! the recovered words, which is the round-trip property Python's own tests assert.
//!
//! One copy, in one place, for the reason two copies of it were a bug waiting to
//! happen: `clients/ssh.rs` used the crate for its workdir while the two flows
//! reproduced CPython, so the same directory name was quoted two ways depending on
//! which transport carried it.

use std::borrow::Cow;

/// `word` as one shell word, as Python's `shlex.quote` writes it.
///
/// Total, as Python's is: a NUL is quoted like any other character here. The
/// callers that must *refuse* a NUL ask [`holds_nul`] first, because refusing is
/// their contract (docs/rust-rewrite-plan.md row 19) rather than the quoting's — and
/// a caller whose input cannot hold one (a workspace id read out of argv) needs no
/// refusal at all and would otherwise be handed an error it cannot explain.
pub fn quote(word: &str) -> Cow<'_, str> {
    if word.is_empty() {
        return Cow::Borrowed("''");
    }
    if word.chars().all(is_shell_safe) {
        return Cow::Borrowed(word);
    }
    Cow::Owned(format!("'{}'", word.replace('\'', "'\"'\"'")))
}

/// The words as one command line, as Python's `shlex.join` writes it.
pub fn join<'a>(words: impl IntoIterator<Item = &'a str>) -> String {
    words
        .into_iter()
        .map(quote)
        .collect::<Vec<Cow<'a, str>>>()
        .join(" ")
}

/// Whether `word` holds a byte no shell word and no argv can carry.
///
/// The one thing quoting cannot fix. Its own question rather than an error arm on
/// [`quote`], because who has to care differs: a `-- <cmd>` from a command line is
/// refused with it (row 19), and a generated script's interpolated workspace id
/// cannot contain one to begin with.
pub(crate) fn holds_nul(word: &str) -> bool {
    word.contains('\0')
}

/// The characters `shlex.quote` leaves unquoted: `[\w@%+=:,./-]` under `re.ASCII`,
/// where `\w` is `[A-Za-z0-9_]`. Anything else — including every non-ASCII
/// character — makes the word need quoting.
fn is_shell_safe(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
        )
}

#[cfg(test)]
mod tests {
    //! The golden vectors are CPython's own `shlex.quote`/`shlex.join` output; the
    //! ones with a single quote or an `@`/`%` in them are exactly where the `shlex`
    //! crate answers differently, which is why this module exists.

    use super::*;

    #[test]
    fn a_safe_word_is_returned_bare() {
        for word in ["claude", "a@b%c", "/tmp/x-1_2.3", "a+b=c:d,e", "main"] {
            assert_eq!(quote(word), word, "{word:?} needed no quoting");
        }
    }

    #[test]
    fn an_empty_word_is_two_quotes() {
        // `shlex.quote("")` is `''`: an empty argument has to survive as one.
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn a_word_needing_quotes_is_single_quoted() {
        assert_eq!(quote("fix the bug"), "'fix the bug'");
        assert_eq!(quote("hi; rm -rf /"), "'hi; rm -rf /'");
        assert_eq!(quote("$HOME"), "'$HOME'");
        // Non-ASCII is not `\w` under `re.ASCII`, so Python quotes it.
        assert_eq!(quote("naïve"), "'naïve'");
    }

    #[test]
    fn an_embedded_quote_is_broken_out_pythons_way() {
        // The `shlex` crate would answer `"it's here"`; CPython answers this, and
        // this is what the harness compares.
        assert_eq!(quote("it's here"), r#"'it'"'"'s here'"#);
        assert_eq!(quote("'"), r#"''"'"''"#);
    }

    #[test]
    fn joining_quotes_each_word_and_separates_with_one_space() {
        assert_eq!(
            join(["claude", "--dangerously-skip-permissions", "fix the bug"]),
            "claude --dangerously-skip-permissions 'fix the bug'"
        );
        assert_eq!(join(Vec::<&str>::new()), "");
    }

    #[test]
    fn a_nul_is_named_rather_than_quoted_away() {
        assert!(holds_nul("a\0b"));
        assert!(!holds_nul("ab"));
    }
}
