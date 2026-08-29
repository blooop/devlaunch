//! JSON written the way CPython's `json.dumps` writes it.
//!
//! Both binaries write documents the cutover checklist compares byte for byte —
//! the `metadata.json` store, the `completions.json` cache, `dl --ls --json`,
//! `dl --completion-data`, the `SOURCE` column's rendering of a source dl cannot
//! read, and the `DEVLAUNCH_TIMING=json` line — so the spelling is part of the
//! contract rather than a style choice. One copy of it in this crate, at the
//! bottom of it, because a second copy is how one of those documents drifts:
//! the timing document was written with `serde_json::to_string` and lost the
//! spacing while its own docstring promised byte-comparability. `dl --ls --json`
//! was the last of them spelled somewhere else, by a formatter of `dl`'s own;
//! devlaunch#346 collapsed that onto [`as_python_writes_it_indented`], so all six
//! are now spelled from here and there is nowhere else to look.
//!
//! That list is hand-maintained, so it is the part of this paragraph that rots.
//! The one a compiler will give you is the callers of [`as_python_writes_it`],
//! [`serialize_as_python`], [`as_python_writes_it_indented`] and
//! [`PythonPrettyFormatter`]; a seventh document that reaches none of them is a
//! second spelling, whatever this paragraph has come to say by then.
//!
//! Below the four layers on purpose: `timing` is the crate root's own module and
//! `flows` sits at the top, so a shared helper either lives here or gets reached
//! for upwards.

use std::io;

use serde::Serialize;

/// The JSON type a value turned out to be, where a typed refusal names it.
///
/// Data for a report, not a sentence: Python's messages name the Python type
/// (`dict`, `str`, `NoneType`), which is a spelling the `dl` binary chooses.
/// One copy here rather than one per boundary — `devpod`'s listing and
/// `metadata`'s store both name kinds, and two identical enums forced the
/// binary to keep two identical renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum JsonKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl JsonKind {
    /// Which kind `value` is.
    pub(crate) fn of(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(_) => Self::Bool,
            serde_json::Value::Number(_) => Self::Number,
            serde_json::Value::String(_) => Self::String,
            serde_json::Value::Array(_) => Self::Array,
            serde_json::Value::Object(_) => Self::Object,
        }
    }
}

/// A JSON value spelled the way Python's `json.dumps` spells it.
///
/// Three surfaces need it and all three are compared byte for byte against the
/// Python build: the `SOURCE` column's rendering of a source dl cannot read, the
/// `completions.json` cache (which the cutover checklist requires both binaries to
/// write identically), and `dl --completion-data`'s echo of it.
///
/// Three differences from `serde_json::to_string`, all of them Python's defaults:
///
/// - separators are `", "` and `": "`, not `","` and `":"`;
/// - everything outside `' '..'~'` is escaped (`ensure_ascii=True`), as `\uXXXX`
///   in lowercase hex, with astral characters written as the surrogate pair
///   Python writes. Note the rule is CPython's printable range and not "non-ASCII":
///   DEL is ASCII and Python escapes it ([`python_writes_it_bare`]);
/// - floats are spelled the way `repr()` spells them
///   ([`PythonFormatter::write_f64`]).
pub(crate) fn as_python_writes_it(value: &serde_json::Value) -> String {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, PythonFormatter);
    if value.serialize(&mut serializer).is_err() {
        return String::new();
    }
    String::from_utf8(out).unwrap_or_default()
}

/// `json.dumps`' default spacing and `ensure_ascii`.
pub(crate) struct PythonFormatter;

impl serde_json::ser::Formatter for PythonFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(if first { b"" } else { b", " })
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(if first { b"" } else { b", " })
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(b": ")
    }

    /// A float spelled the way `repr()` spells it, which is what `json.dumps`
    /// writes.
    ///
    /// serde's writer (ryu) and CPython agree on the digits — both write the
    /// shortest decimal that round-trips — and disagree on the two decisions
    /// *around* them, which is enough to make one line differ:
    ///
    /// - **when to use exponential notation.** ryu switches below `1e-5`
    ///   (`0.00001` for `1e-5`, `1e-6` below that); CPython switches one decade
    ///   earlier, at `1e-05`. A `DEVLAUNCH_TIMING=json` stage that took 40µs is in
    ///   that decade.
    /// - **how to write the exponent.** CPython pads it to two digits and always
    ///   signs it (`1e-06`, `1e+16`); ryu writes `1e-6`.
    ///
    /// The rule is CPython's `float_repr_style`: exponential exactly when the
    /// decimal point would fall at or before the fourth place to the left, or
    /// after the seventeenth to the right — `decpt <= -4 || decpt > 16` in
    /// `PyOS_double_to_string`'s terms — and positional otherwise, with the `.0`
    /// Python gives an integral float.
    fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(python_repr(value).as_bytes())
    }

    /// The run of string bytes serde did not have to escape, since it escapes only
    /// the control characters under `0x20`, `"` and `\`. Everything Python escapes
    /// and serde does not arrives here: every non-ASCII byte, and DEL, which is
    /// ASCII and is above serde's control range. Escaping the rest is
    /// [`write_ensure_ascii`]'s job.
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        write_ensure_ascii(writer, fragment)
    }
}

/// A JSON value spelled the way `json.dumps(value, indent=2)` spells it.
///
/// The indented half of the contract [`as_python_writes_it`] carries for the
/// compact one, and two documents want it. The metadata store writes
/// `metadata.json` with it, through the formatter below rather than through here,
/// because its document is a struct and routing a struct through a
/// [`serde_json::Value`] would put its field order at the mercy of the map. And
/// `dl` writes `dl --ls --json` with it, which is a wire format `wf` parses
/// rather than a rendering choice, so a byte of it is not `dl`'s to change.
///
/// `dl` used to spell that document with a formatter of its own: the third copy
/// of the escaping, and a second copy of the layout delegation with it, which had
/// to stay character-for-character equal to this one for the two documents to go
/// on agreeing with the same Python. It is this function now (devlaunch#346).
///
/// Indented and compact stay two spellings, because that difference is real: this
/// document is laid out over lines and that one is on one line. The escaping and
/// the delegation underneath them are what may not stay two.
pub fn as_python_writes_it_indented(value: &serde_json::Value) -> String {
    let mut out = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut out, PythonPrettyFormatter::default());
    if value.serialize(&mut serializer).is_err() {
        // Not reachable from a `serde_json::Value`, and an empty document is the
        // one answer that cannot be mistaken for a listing.
        return String::new();
    }
    String::from_utf8(out).unwrap_or_default()
}

/// `json.dump(..., indent=2)`, escaping included.
///
/// Two-space indentation is what serde's pretty printer already does, and it puts
/// the `": "` after a key where Python puts it, so the whole of the layout is
/// delegated to it unchanged. What it does not do is Python's `ensure_ascii`,
/// which spells every character outside `' '..'~'` as a `\uXXXX` escape. That is
/// the printable range and not "non-ASCII": DEL is ASCII and Python escapes it
/// too. A branch name with an umlaut in it is enough to make two builds write
/// different bytes for the same data, so the escaping is matched rather than left
/// to chance, and it is matched by calling [`write_ensure_ascii`] rather than by
/// a loop here.
///
/// Only the layout differs from the compact [`PythonFormatter`] — this document
/// is indented and that one is on one line. The float spelling is the same one,
/// forwarded below, so the two cannot come to disagree about a number the way
/// they came to disagree about DEL.
#[derive(Default)]
pub(crate) struct PythonPrettyFormatter<'indent> {
    pretty: serde_json::ser::PrettyFormatter<'indent>,
}

impl serde_json::ser::Formatter for PythonPrettyFormatter<'_> {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        write_ensure_ascii(writer, fragment)
    }

    /// Floats, spelled by the same [`python_repr`] the compact formatter uses.
    ///
    /// Unreachable from either document as they stand, and it moves no byte of
    /// them: `metadata.json`'s only bare number is its `version`, an `i64`, and
    /// every number in `dl --ls --json` sits inside `disk` — which is an object
    /// (`{"exclusiveBytes": u64}`, or `{"atLeastBytes": u64, "unreadable":
    /// usize}`) or `null`, and never a number itself. `unsaved` is an object
    /// holding a bool or a string, and `lastSweep` holds a token and a string.
    ///
    /// It is three lines anyway, because the sentence it replaces was a claim
    /// about callers held in prose, and prose is precisely what this module's
    /// own cautionary tale is about. [`as_python_writes_it_indented`] is `pub`
    /// and takes any [`serde_json::Value`], so the day a float does arrive it is
    /// spelled Python's way rather than ryu's, and nobody has to have remembered
    /// this paragraph.
    fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(python_repr(value).as_bytes())
    }

    // The rest is the pretty printer's layout, delegated unchanged. The nine
    // methods below are exactly the nine `PrettyFormatter` overrides; a tenth
    // delegation `dl`'s deleted copy carried, for `end_object_key`, forwarded the
    // trait's own no-op to a `PrettyFormatter` that does not override it either,
    // which is why the two copies wrote identical bytes despite disagreeing on
    // how many methods the job takes.

    fn begin_array<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_array(writer)
    }

    fn end_array<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_array(writer)
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_array_value(writer, first)
    }

    fn end_array_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_array_value(writer)
    }

    fn begin_object<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object(writer)
    }

    fn end_object<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object(writer)
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object_key(writer, first)
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object_value(writer)
    }

    fn end_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object_value(writer)
    }
}

/// One unescaped run of a JSON string, with Python's `ensure_ascii` applied.
///
/// The escaping half of the spelling, and now the only copy of it anywhere in
/// either binary: [`PythonFormatter`] and [`PythonPrettyFormatter`] differ in
/// layout and call here for the rest, and there is no third formatter left to
/// call anything. There were three hand-written loops once, each having to stay
/// character-for-character equal to the others with nothing holding them there,
/// and the bill came in exactly as you would expect: the gate was wrong about DEL
/// in every copy that was still standing, so devlaunch#349 fixed the same
/// character twice in two files. devlaunch#346 retired the second copy.
///
/// Anything Python does not write as itself becomes `\uXXXX` in lowercase hex,
/// and a character outside the basic plane becomes the two escapes of its UTF-16
/// surrogate pair, because that is what `json.dumps` writes for an emoji.
pub(crate) fn write_ensure_ascii<W>(writer: &mut W, fragment: &str) -> io::Result<()>
where
    W: ?Sized + io::Write,
{
    // The overwhelmingly common case, and the one worth not walking per character.
    // A byte pass rather than a char pass: every byte of a multi-byte character is
    // `0x80` or above, so any of them fails the test and drops to the slow path.
    if fragment.bytes().map(char::from).all(python_writes_it_bare) {
        return writer.write_all(fragment.as_bytes());
    }
    let mut bare_from = 0;
    let mut units = [0u16; 2];
    for (at, character) in fragment.char_indices() {
        if python_writes_it_bare(character) {
            continue;
        }
        writer.write_all(&fragment.as_bytes()[bare_from..at])?;
        bare_from = at + character.len_utf8();
        for unit in character.encode_utf16(&mut units) {
            write!(writer, "\\u{unit:04x}")?;
        }
    }
    writer.write_all(&fragment.as_bytes()[bare_from..])
}

/// Whether `json.dumps` writes this character as itself.
///
/// CPython's `S_CHAR`, transcribed: space through `~`, less the quote and the
/// backslash. `is_ascii()` is the tempting spelling and the wrong one by exactly
/// one character — **DEL (`U+007F`)**, which is ASCII, is not printable, and is
/// the single non-printable ASCII character serde hands to the fragment writer
/// rather than escaping from its own table. Three copies of this escaper spelled
/// the gate `is_ascii()` and all three wrote a raw `0x7f` where Python wrote six
/// characters (devlaunch#349).
///
/// The quote and the backslash are unreachable either way, because serde's own
/// table escapes both before a fragment is cut, so the two arms of that choice are
/// only distinguishable if serde ever stops. They are excluded anyway, because
/// that is the arm that fails safe: excluded, a stray `"` comes out as `\u0022` —
/// valid JSON, spelled differently from Python's `\"`, and never doubled, since
/// this loop only ever sees a character once. Included, it comes out raw, which
/// terminates the string early and writes a document nothing can parse. Costing
/// two comparisons per byte to turn "silently invalid" into "valid and slightly
/// oddly spelled" is worth it, and it also makes the predicate answer its own
/// question honestly: `json.dumps` does not write a bare quote.
fn python_writes_it_bare(character: char) -> bool {
    matches!(character, ' '..='~') && character != '"' && character != '\\'
}

/// Anything `Serialize`, spelled the same way.
///
/// The document types that need this are structs rather than
/// [`serde_json::Value`]s, and routing them through a `Value` first would put
/// their field order at the mercy of the map implementation. Serializing the
/// struct keeps declaration order, which is the order Python's dict was built in.
pub(crate) fn serialize_as_python<T: Serialize>(value: &T) -> Option<String> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, PythonFormatter);
    value.serialize(&mut serializer).ok()?;
    String::from_utf8(out).ok()
}

/// One finite `f64`, in the digits and the shape CPython's `repr` gives it.
///
/// Only finite values reach here: `serde_json` writes a NaN or an infinity as
/// `null` without consulting the formatter. Written for the reader of
/// [`PythonFormatter::write_f64`], whose docstring carries the rule.
fn python_repr(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    // `{:e}` is the shortest round-trip mantissa with the decimal exponent beside
    // it, which is the pair the decision below is made on.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("LowerExp writes the exponent after an `e`");
    let exponent: i32 = exponent
        .parse()
        .expect("LowerExp writes an integer exponent");
    // Where the decimal point falls, counted CPython's way: `value` is
    // `0.<digits> * 10^decpt`, so one more than `{:e}`'s exponent.
    let decimal_point = exponent + 1;
    if decimal_point <= -4 || decimal_point > 16 {
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    let positional = format!("{value}");
    if positional.contains('.') {
        positional
    } else {
        // Rust's Display drops the fraction of an integral float; Python's repr
        // keeps `.0` so the value still reads as a float.
        format!("{positional}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every case here was produced by running `json.dumps(value)` under the
    /// frozen Python build (3.14) and pasting the answer in. Nothing in the
    /// expectations was read off this module.
    #[test]
    fn floats_are_spelled_the_way_json_dumps_spells_them() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (1e-7, "1e-07"),
            (1e-6, "1e-06"),
            (6e-6, "6e-06"),
            (1.5e-6, "1.5e-06"),
            // The decade ryu writes positionally and CPython does not.
            (1e-5, "1e-05"),
            (9.9e-5, "9.9e-05"),
            // And the first one they agree on.
            (1e-4, "0.0001"),
            (0.000123, "0.000123"),
            (0.001, "0.001"),
            (0.5, "0.5"),
            (1.0, "1.0"),
            (5.0, "5.0"),
            (30.0, "30.0"),
            (1.234567, "1.234567"),
            (123.456789, "123.456789"),
            // The upper switch, which no duration reaches and the rule covers anyway.
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1e17, "1e+17"),
        ];
        for (value, expected) in cases {
            assert_eq!(
                as_python_writes_it(&serde_json::json!(value)),
                *expected,
                "for {value:e}"
            );
        }
    }

    #[test]
    fn a_float_inside_a_document_gets_the_same_spelling() {
        // Through the object path as well as the bare value, because that is where
        // the timing document's numbers live.
        assert_eq!(
            as_python_writes_it(&serde_json::json!({ "seconds": 4e-5, "total": 2.0 })),
            r#"{"seconds": 4e-05, "total": 2.0}"#
        );
    }

    /// The escaping half on its own, at the seam both formatters now share.
    ///
    /// Every expectation is the inside of what `json.dumps` wrote for the same
    /// text under the frozen Python build (3.14) — the quotes stripped, since a
    /// fragment is what falls between them.
    #[test]
    fn ensure_ascii_writes_what_json_dumps_writes_between_the_quotes() {
        let cases: &[(&str, &str)] = &[
            // Nothing to do: the safe path every ASCII document takes.
            ("plain/path-1_2.3", "plain/path-1_2.3"),
            ("feature/br\u{fc}nch", "feature/br\\u00fcnch"),
            // DEL, the one non-printable ASCII character serde hands here rather
            // than escaping itself. Python escapes it like any other character
            // outside `' '..'~'`.
            ("\u{7f}", "\\u007f"),
            ("a\u{7f}b", "a\\u007fb"),
            // Astral, so Python writes the UTF-16 surrogate pair rather than one
            // escape.
            ("\u{1f680}", "\\ud83d\\ude80"),
            // Mixed, so the ASCII runs on either side have to survive intact.
            ("a\u{1f680}b\u{e9}c", "a\\ud83d\\ude80b\\u00e9c"),
            ("", ""),
        ];
        for (fragment, expected) in cases {
            let mut written = Vec::new();
            write_ensure_ascii(&mut written, fragment).expect("a Vec never fails to write");
            assert_eq!(
                String::from_utf8(written).expect("escaping writes ASCII"),
                *expected,
                "for {fragment:?}"
            );
        }
    }

    /// The classes serde escapes before the shared escaper is ever reached, at
    /// the document.
    ///
    /// [`write_ensure_ascii`] only ever sees the runs serde left alone, so the
    /// quotes, the backslash and everything under `0x20` are spelled by serde's
    /// own table rather than by anything in this module — which is exactly why
    /// they want pinning here: nothing else in the crate asserts that the table
    /// happens to agree with Python's, and the escaper extraction is the sort of
    /// edit that invites a hand-written table beside it. `/` is here because
    /// Python leaves it bare and a lot of JSON writers do not, and `U+2028`/
    /// `U+2029` because they are the non-ASCII pair a JavaScript-flavoured
    /// escaper singles out and Python does not.
    ///
    /// Expectation from `json.dumps` under the frozen Python build.
    #[test]
    fn the_escapes_serde_writes_are_the_ones_python_writes() {
        assert_eq!(
            as_python_writes_it(&serde_json::json!({
                "branch": "a\"b\\c\nd\te\rf\u{8}g\u{c}h",
                "tags": ["\u{0}\u{1}\u{1f}", "/slash/", "\u{2028}\u{2029}"],
            })),
            concat!(
                r#"{"branch": "a\"b\\c\nd\te\rf\bg\fh", "#,
                r#""tags": ["\u0000\u0001\u001f", "/slash/", "\u2028\u2029"]}"#,
            )
        );
    }

    /// Every ASCII character at once, against the one line Python wrote for the
    /// same string.
    ///
    /// The per-class tests above each pin a class someone thought to name, which
    /// is how DEL stayed wrong through three copies of this escaper: it belongs
    /// to no class anyone had written down. Serde escapes the C0 controls, `"`
    /// and `\`; Python escapes those plus everything outside `' '..'~'`; and the
    /// gap between the two descriptions is exactly one character wide. Sweeping
    /// the whole of `U+0000..=U+007F` in one string closes the gap in both
    /// directions at once — nothing bare that Python escapes, and nothing escaped
    /// that Python leaves bare.
    ///
    /// The expectation is the literal `json.dumps` printed for
    /// `''.join(chr(c) for c in range(0x80))`, pasted whole.
    #[test]
    fn every_ascii_character_is_spelled_the_way_json_dumps_spells_it() {
        let all_of_ascii: String = (0u8..=0x7f).map(char::from).collect();
        assert_eq!(
            as_python_writes_it(&serde_json::json!(all_of_ascii)),
            r##""\u0000\u0001\u0002\u0003\u0004\u0005\u0006\u0007\b\t\n\u000b\f\r\u000e\u000f\u0010\u0011\u0012\u0013\u0014\u0015\u0016\u0017\u0018\u0019\u001a\u001b\u001c\u001d\u001e\u001f !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u007f""##
        );
    }

    #[test]
    fn a_compact_document_escapes_the_same_way() {
        // What `json.dumps({"branch": "feature/brünch"})` printed.
        assert_eq!(
            as_python_writes_it(&serde_json::json!({ "branch": "feature/br\u{fc}nch" })),
            r#"{"branch": "feature/br\u00fcnch"}"#
        );
        // And `json.dumps(["\U0001f680"])`, where the rocket is one code point
        // and two escapes.
        assert_eq!(
            as_python_writes_it(&serde_json::json!(["\u{1f680}"])),
            r#"["\ud83d\ude80"]"#
        );
    }

    /// The indented spelling's layout, at the shape `dl --ls --json` writes: a
    /// list of objects, carrying the three value types the listing puts in one.
    ///
    /// Expectation is the literal `json.dumps` printed for the same data with
    /// `indent=2`. Layout is the half of this spelling that is not escaping, and
    /// it is the half a delegating formatter gets wrong quietly: serde's pretty
    /// printer agrees with Python on the two-space indent and on the `": "` after
    /// a key, and the pin is what holds it to going on agreeing.
    #[test]
    fn an_indented_document_is_laid_out_the_way_json_dumps_lays_it_out() {
        let document = serde_json::json!([{ "id": "ws", "devlaunch": true, "unsaved": null }]);
        assert_eq!(
            as_python_writes_it_indented(&document),
            "[\n  {\n    \"id\": \"ws\",\n    \"devlaunch\": true,\n    \"unsaved\": null\n  }\n]"
        );
        // Python indents nothing it does not have to: an empty list is two
        // characters under `indent=2`, not two characters and a newline.
        assert_eq!(as_python_writes_it_indented(&serde_json::json!([])), "[]");
    }

    /// The escaping reaching the indented document through a nest, rather than at
    /// a bare string where no layout is ever asked for.
    ///
    /// Two levels deep is where a formatter that delegates layout to serde and
    /// escaping to this module has to hand off correctly in both directions at
    /// once. The rocket is astral, so Python writes the two escapes of its UTF-16
    /// surrogate pair rather than one escape. Expectation is the literal
    /// `json.dumps` printed for the same data with `indent=2`.
    #[test]
    fn an_indented_document_escapes_through_the_nesting() {
        assert_eq!(
            as_python_writes_it_indented(
                &serde_json::json!({ "a": [1, "\u{1f680}"], "b": { "c": null } })
            ),
            "{\n  \"a\": [\n    1,\n    \"\\ud83d\\ude80\"\n  ],\n  \"b\": {\n    \"c\": null\n  }\n}"
        );
    }

    /// DEL at the indented document.
    ///
    /// The one non-printable ASCII character serde hands to the fragment writer
    /// rather than escaping from its own table, so it is the character that says
    /// whether the indented spelling reaches this module's escaping or carries a
    /// copy that spelled the gate `is_ascii()`. Expectation from
    /// `json.dumps(..., indent=2)`.
    #[test]
    fn an_indented_document_escapes_del_the_way_json_dumps_does() {
        assert_eq!(
            as_python_writes_it_indented(&serde_json::json!("a\u{7f}b")),
            "\"a\\u007fb\""
        );
    }

    /// The whole ASCII range at once, at the indented seam.
    ///
    /// The pin the crate's other two spellings already had and this one did not:
    /// the compact formatter is swept by
    /// [`every_ascii_character_is_spelled_the_way_json_dumps_spells_it`] and
    /// `metadata.json` by `the_indent_two_document_spells_every_ascii_character_pythons_way`,
    /// and both exist because the per-class tests each pin a class someone
    /// thought to name — which is how DEL stayed wrong through three copies of
    /// the escaper. Sweeping closes the gap in both directions at once: nothing
    /// bare that Python escapes, nothing escaped that it leaves bare.
    ///
    /// Inside an object rather than at a bare string, because a bare string asks
    /// for no layout at all and would make this the compact sweep under a second
    /// name. Expectation is the literal `json.dumps` printed for
    /// `{"branch": ''.join(chr(c) for c in range(0x80))}` with `indent=2`, under
    /// the frozen Python build (3.14).
    #[test]
    fn an_indented_document_spells_every_ascii_character_the_way_json_dumps_does() {
        let all_of_ascii: String = (0u8..=0x7f).map(char::from).collect();
        assert_eq!(
            as_python_writes_it_indented(&serde_json::json!({ "branch": all_of_ascii })),
            concat!(
                "{\n  \"branch\": \"",
                r##"\u0000\u0001\u0002\u0003\u0004\u0005\u0006\u0007\b\t\n\u000b\f\r\u000e\u000f\u0010\u0011\u0012\u0013\u0014\u0015\u0016\u0017\u0018\u0019\u001a\u001b\u001c\u001d\u001e\u001f !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u007f"##,
                "\"\n}",
            )
        );
    }

    /// The indented spelling's floats, which are the compact one's.
    ///
    /// No document that reaches [`as_python_writes_it_indented`] carries a float
    /// today — see [`PythonPrettyFormatter::write_f64`] for why — so this pins
    /// the override rather than a wire, and it is here because an override with
    /// nothing measuring it is how the two formatters would drift apart again.
    /// `1e-05` is the decade where ryu and CPython disagree, `1e+16` the exponent
    /// spelling they disagree on, and `-0.0` and `1.0` the shapes Rust's `Display`
    /// writes without the fraction. Expectation is the literal `json.dumps`
    /// printed for the same list with `indent=2`.
    #[test]
    fn an_indented_document_spells_floats_the_way_json_dumps_does() {
        assert_eq!(
            as_python_writes_it_indented(&serde_json::json!([1e-5, 1e16, -0.0, 1.0])),
            "[\n  1e-05,\n  1e+16,\n  -0.0,\n  1.0\n]"
        );
    }

    #[test]
    fn integers_are_untouched() {
        // The listing's numbers are `disk`'s, and Python writes them bare. `disk`
        // itself is an object or null, never a number.
        assert_eq!(
            as_python_writes_it(&serde_json::json!({ "exclusiveBytes": 4096 })),
            r#"{"exclusiveBytes": 4096}"#
        );
    }
}
