//! JSON written the way CPython's `json.dumps` writes it.
//!
//! Both binaries write documents the cutover checklist compares byte for byte —
//! the `completions.json` cache, `dl --ls --json`, `dl --completion-data`, the
//! `SOURCE` column's rendering of a source dl cannot read, and the
//! `DEVLAUNCH_TIMING=json` line — so the spelling is part of the contract rather
//! than a style choice. One copy of it, at the bottom of the crate, because a
//! second copy is how one of those documents drifts: the timing document was
//! written with `serde_json::to_string` and lost the spacing while its own
//! docstring promised byte-comparability.
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
/// - non-ASCII is escaped (`ensure_ascii=True`), as `\uXXXX` in lowercase hex,
///   with astral characters written as the surrogate pair Python writes;
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

    /// The run of string bytes serde did not have to escape — which includes every
    /// non-ASCII byte, since serde escapes only the control characters, `"` and
    /// `\`. Python escapes the rest too, so this is where `ensure_ascii` lives.
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        if fragment.is_ascii() {
            return writer.write_all(fragment.as_bytes());
        }
        let mut buffer = [0u16; 2];
        for character in fragment.chars() {
            if character.is_ascii() {
                writer.write_all(character.encode_utf8(&mut [0u8; 4]).as_bytes())?;
            } else {
                for unit in character.encode_utf16(&mut buffer) {
                    writer.write_all(format!("\\u{unit:04x}").as_bytes())?;
                }
            }
        }
        Ok(())
    }
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

    #[test]
    fn integers_are_untouched() {
        // `disk` in `dl --ls --json` is an integer, and Python writes it bare.
        assert_eq!(
            as_python_writes_it(&serde_json::json!({ "disk": 4096 })),
            r#"{"disk": 4096}"#
        );
    }
}
