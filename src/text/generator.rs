//! The OpenStep/GNUStep text generator.

use std::io::Write;

use crate::date::Date;
use crate::error::Result;
use crate::format::Format;
use crate::scalar;
use crate::text::tables::{CharSet, GS_QUOTABLE, OS_QUOTABLE};
use crate::value::{Dictionary, Integer, Value};

/// Writes `value` as a text plist in the given dialect.
///
/// The two dialects differ only in the quoting table and in GNUStep's typed
/// `<*...>` scalars; any non-GNUStep `format` selects OpenStep emission. A
/// non-empty `indent` switches the key delimiter to ` = ` and breaks every
/// entry onto its own indented line. Every [`Value`] serializes; astral-plane
/// characters emit a single 5-6 digit `\U` escape that deliberately does not
/// round-trip.
///
/// # Errors
///
/// Returns [`Error::Io`](crate::Error::Io) when the writer fails; nothing
/// else can fail.
pub(crate) fn generate<W: Write>(
    writer: &mut W,
    value: &Value,
    format: Format,
    indent: &str,
) -> Result<()> {
    let gnustep = format == Format::GnuStep;
    let mut generator = Generator {
        writer,
        gnustep,
        table: if gnustep { GS_QUOTABLE } else { OS_QUOTABLE },
        indent,
        kv_delimiter: if indent.is_empty() { "=" } else { " = " },
        depth: 0,
    };
    generator.write_value(value)
}

struct Generator<'a, W> {
    writer: &'a mut W,
    gnustep: bool,
    table: CharSet,
    indent: &'a str,
    kv_delimiter: &'a str,
    depth: usize,
}

impl<W: Write> Generator<'_, W> {
    fn write_str(&mut self, s: &str) -> Result<()> {
        self.writer.write_all(s.as_bytes())?;
        Ok(())
    }

    fn write_indent(&mut self) -> Result<()> {
        if self.indent.is_empty() {
            return Ok(());
        }
        self.write_str("\n")?;
        for _ in 0..self.depth {
            let unit = self.indent;
            self.write_str(unit)?;
        }
        Ok(())
    }

    fn write_value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Dictionary(dict) => self.write_dictionary(dict),
            Value::Array(values) => self.write_array(values),
            Value::String(s) => self.write_quoted_string(s),
            Value::Integer(integer) => self.write_integer(*integer),
            Value::Real(real) => self.write_real(real.value()),
            Value::Boolean(boolean) => self.write_boolean(*boolean),
            Value::Data(data) => self.write_data(data),
            Value::Date(date) => self.write_date(*date),
            Value::Uid(uid) => {
                let dict = Dictionary::from([(
                    "CF$UID".to_owned(),
                    Value::Integer(Integer::Unsigned(uid.get())),
                )]);
                self.write_dictionary(&dict)
            }
        }
    }

    fn write_dictionary(&mut self, dict: &Dictionary) -> Result<()> {
        self.write_str("{")?;
        self.depth += 1;
        for (key, value) in dict {
            self.write_indent()?;
            self.write_quoted_string(key)?;
            let delimiter = self.kv_delimiter;
            self.write_str(delimiter)?;
            self.write_value(value)?;
            self.write_str(";")?;
        }
        self.depth -= 1;
        self.write_indent()?;
        self.write_str("}")
    }

    fn write_array(&mut self, values: &[Value]) -> Result<()> {
        self.write_str("(")?;
        self.depth += 1;
        for value in values {
            self.write_indent()?;
            self.write_value(value)?;
            self.write_str(",")?;
        }
        self.depth -= 1;
        self.write_indent()?;
        self.write_str(")")
    }

    fn write_quoted_string(&mut self, s: &str) -> Result<()> {
        let quoted = quote_string(self.table, s);
        self.write_str(&quoted)
    }

    fn write_integer(&mut self, integer: Integer) -> Result<()> {
        if self.gnustep {
            self.write_str("<*I")?;
        }
        write!(self.writer, "{integer}")?;
        if self.gnustep {
            self.write_str(">")?;
        }
        Ok(())
    }

    fn write_real(&mut self, value: f64) -> Result<()> {
        if self.gnustep {
            self.write_str("<*R")?;
        }
        let formatted = scalar::format_f64(value);
        self.write_str(&formatted)?;
        if self.gnustep {
            self.write_str(">")?;
        }
        Ok(())
    }

    fn write_boolean(&mut self, boolean: bool) -> Result<()> {
        let token = match (self.gnustep, boolean) {
            (true, true) => "<*BY>",
            (true, false) => "<*BN>",
            (false, true) => "1",
            (false, false) => "0",
        };
        self.write_str(token)
    }

    /// Lowercase hex with a single space after every fourth byte, in both
    /// dialects — the generator never emits `<[base64]>`.
    fn write_data(&mut self, data: &[u8]) -> Result<()> {
        self.write_str("<")?;
        let mut chunks = data.chunks(4).peekable();
        while let Some(chunk) = chunks.next() {
            for byte in chunk {
                write!(self.writer, "{byte:02x}")?;
            }
            if chunks.peek().is_some() {
                self.write_str(" ")?;
            }
        }
        self.write_str(">")
    }

    fn write_date(&mut self, date: Date) -> Result<()> {
        let formatted = date.format_text_layout();
        if self.gnustep {
            self.write_str("<*D")?;
            self.write_str(&formatted)?;
            self.write_str(">")
        } else {
            self.write_quoted_string(&formatted)
        }
    }
}

/// Quotes a string for text output: quotes when any character is in the
/// dialect table or above U+007F; `\a \b \v \f \\ \"` symbolically, TAB/CR/LF
/// and other controls raw, U+0080-U+00FF as 3-digit octal, and everything
/// higher as `\U` plus lowercase hex padded to at least four digits.
fn quote_string(table: CharSet, s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_owned();
    }
    let mut body = String::with_capacity(s.len());
    let mut quote = false;
    for c in s.chars() {
        let code = u32::from(c);
        if code > 0xFF {
            quote = true;
            body.push_str("\\U");
            let hex = format!("{code:x}");
            for _ in hex.len()..4 {
                body.push('0');
            }
            body.push_str(&hex);
        } else if code > 0x7F {
            quote = true;
            body.push('\\');
            // Always exactly three octal digits for 0o200..=0o377.
            for shift in [6, 3, 0] {
                body.push(char::from_digit((code >> shift) & 7, 8).unwrap_or('0'));
            }
        } else {
            if table.contains_char(c) {
                quote = true;
            }
            match c {
                '\u{07}' => body.push_str("\\a"),
                '\u{08}' => body.push_str("\\b"),
                '\u{0B}' => body.push_str("\\v"),
                '\u{0C}' => body.push_str("\\f"),
                '\\' => body.push_str("\\\\"),
                '"' => body.push_str("\\\""),
                _ => body.push(c),
            }
        }
    }
    if quote { format!("\"{body}\"") } else { body }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

    use std::io;

    use crate::date::Date;
    use crate::error::Error;
    use crate::format::Format;
    use crate::text::{generate, parse};
    use crate::uid::Uid;
    use crate::value::{Real, Value};

    fn render(value: &Value, format: Format, indent: &str) -> String {
        let mut out = Vec::new();
        generate(&mut out, value, format, indent).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn os(value: &Value) -> String {
        render(value, Format::OpenStep, "")
    }

    fn gs(value: &Value) -> String {
        render(value, Format::GnuStep, "")
    }

    fn s(v: &str) -> Value {
        Value::from(v)
    }

    fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn fixture_date() -> Value {
        Value::Date(Date::parse_text_layout("2013-11-27 00:34:00 +0000").unwrap())
    }

    fn sparse_bundle() -> Value {
        dict([
            ("CFBundleInfoDictionaryVersion", s("6.0")),
            ("band-size", Value::from(8_388_608u64)),
            ("bundle-backingstore-version", Value::from(1i64)),
            (
                "diskimage-bundle-type",
                s("com.apple.diskimage.sparsebundle"),
            ),
            ("size", Value::from(4_398_046_511_104u64)),
        ])
    }

    #[test]
    fn sparse_bundle_goldens_pin_the_quoting_table_delta() {
        assert_eq!(
            os(&sparse_bundle()),
            "{CFBundleInfoDictionaryVersion=\"6.0\";\"band-size\"=8388608;\"bundle-backingstore-version\"=1;\"diskimage-bundle-type\"=\"com.apple.diskimage.sparsebundle\";size=4398046511104;}"
        );
        let gs_golden = "{CFBundleInfoDictionaryVersion=6.0;band-size=<*I8388608>;bundle-backingstore-version=<*I1>;diskimage-bundle-type=com.apple.diskimage.sparsebundle;size=<*I4398046511104>;}";
        assert_eq!(gs(&sparse_bundle()), gs_golden);
        // The GNUStep document round-trips at the value layer.
        let (parsed, format) = parse(gs_golden.as_bytes()).unwrap();
        assert_eq!(format, Format::GnuStep);
        assert_eq!(parsed, sparse_bundle());
    }

    #[test]
    fn indent_mode_matches_example_marshal_gnustep_byte_for_byte() {
        let expected = "{\n\tCFBundleInfoDictionaryVersion = 6.0;\n\tband-size = <*I8388608>;\n\tbundle-backingstore-version = <*I1>;\n\tdiskimage-bundle-type = com.apple.diskimage.sparsebundle;\n\tsize = <*I4398046511104>;\n}";
        assert_eq!(render(&sparse_bundle(), Format::GnuStep, "\t"), expected);
    }

    #[test]
    fn indent_mode_layout_details() {
        // Empty containers still take the closing-token indent break.
        assert_eq!(render(&dict([]), Format::OpenStep, "\t"), "{\n}");
        assert_eq!(
            render(&Value::Array(vec![]), Format::OpenStep, "\t"),
            "(\n)"
        );
        // Scalars at the root print bare.
        assert_eq!(render(&s("x"), Format::OpenStep, "\t"), "x");
        // Nested entries indent by depth; arrays keep `,` after each element.
        assert_eq!(
            render(
                &dict([("a", Value::Array(vec![s("b")]))]),
                Format::OpenStep,
                "  "
            ),
            "{\n  a = (\n    b,\n  );\n}"
        );
        // Without indent there are no newlines at all.
        assert_eq!(os(&dict([])), "{}");
        assert_eq!(os(&Value::Array(vec![])), "()");
    }

    #[test]
    fn funny_characters_golden_is_identical_in_both_dialects() {
        let value = dict([
            ("\u{7}", s("\u{8}")),
            ("\t\r", s("\n")),
            ("\u{B}", s("\u{C}")),
            ("\\", s("\"")),
            ("\u{C8}", s("wat")),
            ("\u{100}", s("hundred")),
        ]);
        let golden = "{\"\\a\"=\"\\b\";\"\t\r\"=\"\n\";\"\\v\"=\"\\f\";\"\\\\\"=\"\\\"\";\"\\310\"=wat;\"\\U0100\"=hundred;}";
        assert_eq!(os(&value), golden);
        assert_eq!(gs(&value), golden);
        assert_eq!(parse(golden.as_bytes()).unwrap().0, value);
    }

    #[test]
    fn string_quoting_decision_pins() {
        // Bare under both tables.
        assert_eq!(os(&s("wat")), "wat");
        assert_eq!(os(&s("4398046511104")), "4398046511104");
        // OS quotes `.` `-` `_` etc.; GS leaves them bare.
        assert_eq!(os(&s("A.B.C.A1")), "\"A.B.C.A1\"");
        assert_eq!(gs(&s("A.B.C.A1")), "A.B.C.A1");
        // The apostrophe is quoted in both dialects.
        assert_eq!(os(&s("'")), "\"'\"");
        assert_eq!(gs(&s("'")), "\"'\"");
        // Empty string is exactly "".
        assert_eq!(os(&s("")), "\"\"");
        assert_eq!(gs(&s("")), "\"\"");
        // Bug-compatible: OS leaves `;` unquoted and cannot re-parse it.
        assert_eq!(os(&s(";")), ";");
        assert!(parse(b";").is_err());
        assert_eq!(gs(&s(";")), "\";\"");
        // BMP runes above U+00FF become 4-digit lowercase \U escapes.
        assert_eq!(
            os(&Value::Array(vec![s("Hello, ASCII"), s("Hello, 世界")])),
            "(\"Hello, ASCII\",\"Hello, \\U4e16\\U754c\",)"
        );
    }

    #[test]
    fn astral_runes_emit_one_wide_escape_and_do_not_round_trip() {
        let original = dict([("e", s("grin \u{1F600} end"))]);
        for format in [Format::OpenStep, Format::GnuStep] {
            let rendered = render(&original, format, "");
            assert_eq!(rendered, "{e=\"grin \\U1f600 end\";}");
            let (reparsed, _) = parse(rendered.as_bytes()).unwrap();
            // The parser reads only four hex digits: \U1f60 + literal '0'.
            assert_ne!(reparsed, original);
            assert_eq!(reparsed, dict([("e", s("grin \u{1F60}0 end"))]));
        }
    }

    #[test]
    fn integer_emission_per_dialect() {
        assert_eq!(os(&Value::from(-1i64)), "-1");
        assert_eq!(gs(&Value::from(-1i64)), "<*I-1>");
        assert_eq!(os(&Value::from(u64::MAX)), "18446744073709551615");
        let signed = Value::Array(
            [-1i64, -127, -255, -32767, -65535, i64::MIN]
                .into_iter()
                .map(Value::from)
                .collect(),
        );
        assert_eq!(
            os(&signed),
            "(-1,-127,-255,-32767,-65535,-9223372036854775808,)"
        );
        assert_eq!(
            gs(&signed),
            "(<*I-1>,<*I-127>,<*I-255>,<*I-32767>,<*I-65535>,<*I-9223372036854775808>,)"
        );
        let unsigned = Value::Array(
            [
                255u64,
                4095,
                65535,
                1_048_575,
                16_777_215,
                268_435_455,
                4_294_967_295,
                9_223_372_036_854_775_807,
                16_045_690_985_305_262_846,
            ]
            .into_iter()
            .map(Value::from)
            .collect(),
        );
        assert_eq!(
            os(&unsigned),
            "(255,4095,65535,1048575,16777215,268435455,4294967295,9223372036854775807,16045690985305262846,)"
        );
        assert_eq!(
            gs(&unsigned),
            "(<*I255>,<*I4095>,<*I65535>,<*I1048575>,<*I16777215>,<*I268435455>,<*I4294967295>,<*I9223372036854775807>,<*I16045690985305262846>,)"
        );
    }

    #[test]
    fn real_emission_uses_64_bit_format_always() {
        assert_eq!(os(&Value::from(std::f64::consts::PI)), "3.141592653589793");
        assert_eq!(
            gs(&Value::from(std::f64::consts::PI)),
            "<*R3.141592653589793>"
        );
        assert_eq!(os(&Value::from(f64::INFINITY)), "+Inf");
        assert_eq!(gs(&Value::from(f64::INFINITY)), "<*R+Inf>");
        assert_eq!(os(&Value::from(f64::NEG_INFINITY)), "-Inf");
        assert_eq!(gs(&Value::from(f64::NEG_INFINITY)), "<*R-Inf>");
        assert_eq!(os(&Value::from(f64::NAN)), "NaN");
        assert_eq!(gs(&Value::from(f64::NAN)), "<*RNaN>");
        // Narrow reals format with full 64-bit precision regardless.
        let bitness = Value::Array(vec![
            Value::from(Real::from(f32::MAX)),
            Value::from(f64::MAX),
        ]);
        assert_eq!(
            os(&bitness),
            "(3.4028234663852886e+38,1.7976931348623157e+308,)"
        );
        assert_eq!(
            gs(&bitness),
            "(<*R3.4028234663852886e+38>,<*R1.7976931348623157e+308>,)"
        );
    }

    #[test]
    fn boolean_emission_per_dialect() {
        assert_eq!(os(&Value::from(true)), "1");
        assert_eq!(os(&Value::from(false)), "0");
        assert_eq!(gs(&Value::from(true)), "<*BY>");
        assert_eq!(gs(&Value::from(false)), "<*BN>");
    }

    #[test]
    fn data_emits_lowercase_hex_grouped_four_bytes_per_space_in_both_dialects() {
        assert_eq!(os(&Value::Data(vec![])), "<>");
        assert_eq!(os(&Value::Data(b"hello".to_vec())), "<68656c6c 6f>");
        assert_eq!(os(&Value::Data(vec![1, 2, 3, 4])), "<01020304>");
        // GNUStep never writes <[base64]> — read-side only.
        assert_eq!(gs(&Value::Data(b"hello".to_vec())), "<68656c6c 6f>");
        let arrays = Value::Array(vec![
            Value::Data(b"Hello".to_vec()),
            Value::Data(b"World".to_vec()),
        ]);
        assert_eq!(gs(&arrays), "(<48656c6c 6f>,<576f726c 64>,)");
        assert_eq!(
            os(&Value::Data(vec![
                0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x01
            ])),
            "<deadbeef cafebabe 01>"
        );
    }

    #[test]
    fn date_emission_per_dialect() {
        assert_eq!(os(&fixture_date()), "\"2013-11-27 00:34:00 +0000\"");
        assert_eq!(gs(&fixture_date()), "<*D2013-11-27 00:34:00 +0000>");
        // OpenStep dates parse back as strings; GNUStep dates as dates.
        assert_eq!(
            parse(b"\"2013-11-27 00:34:00 +0000\"").unwrap().0,
            s("2013-11-27 00:34:00 +0000")
        );
        assert_eq!(
            parse(b"<*D2013-11-27 00:34:00 +0000>").unwrap().0,
            fixture_date()
        );
    }

    #[test]
    fn uid_emits_the_cf_uid_dictionary_with_dialect_appropriate_integers() {
        let uids = Value::Array(
            [255u64, 65535, 16_777_215, 4_294_967_295, 1_099_511_627_775]
                .into_iter()
                .map(|n| Value::Uid(Uid::from(n)))
                .collect(),
        );
        let os_golden = "({CF$UID=255;},{CF$UID=65535;},{CF$UID=16777215;},{CF$UID=4294967295;},{CF$UID=1099511627775;},)";
        let gs_golden = "({CF$UID=<*I255>;},{CF$UID=<*I65535>;},{CF$UID=<*I16777215>;},{CF$UID=<*I4294967295>;},{CF$UID=<*I1099511627775>;},)";
        assert_eq!(os(&uids), os_golden);
        assert_eq!(gs(&uids), gs_golden);
        assert_eq!(
            os(&dict([("identifier", Value::Uid(Uid::from(1024)))])),
            "{identifier={CF$UID=1024;};}"
        );
        assert_eq!(
            gs(&dict([("identifier", Value::Uid(Uid::from(1024)))])),
            "{identifier={CF$UID=<*I1024>;};}"
        );
    }

    #[test]
    fn map_of_all_variations_goldens() {
        let value = dict([
            (
                "booleans",
                Value::Array(vec![Value::from(true), Value::from(false)]),
            ),
            ("data", Value::Data(vec![1, 2, 3, 4])),
            ("date", fixture_date()),
            (
                "floats",
                Value::Array(vec![Value::from(Real::from(32.0f32)), Value::from(64.0f64)]),
            ),
            (
                "intarray",
                Value::Array(
                    [1i64, 8, 16, 32, 64]
                        .into_iter()
                        .map(Value::from)
                        .chain([2u64, 9, 17, 33, 65].into_iter().map(Value::from))
                        .collect(),
                ),
            ),
            (
                "strings",
                Value::Array(vec![s("Hello, ASCII"), s("Hello, 世界")]),
            ),
        ]);
        assert_eq!(
            os(&value),
            "{booleans=(1,0,);data=<01020304>;date=\"2013-11-27 00:34:00 +0000\";floats=(32,64,);intarray=(1,8,16,32,64,2,9,17,33,65,);strings=(\"Hello, ASCII\",\"Hello, \\U4e16\\U754c\",);}"
        );
        assert_eq!(
            gs(&value),
            "{booleans=(<*BY>,<*BN>,);data=<01020304>;date=<*D2013-11-27 00:34:00 +0000>;floats=(<*R32>,<*R64>,);intarray=(<*I1>,<*I8>,<*I16>,<*I32>,<*I64>,<*I2>,<*I9>,<*I17>,<*I33>,<*I65>,);strings=(\"Hello, ASCII\",\"Hello, \\U4e16\\U754c\",);}"
        );
    }

    #[test]
    fn anonymous_embeds_golden_pins_nested_sorting_and_quoting() {
        let value = dict([
            (
                "EmbedB",
                dict([
                    ("FieldA", s("A.B.C.A1")),
                    ("FieldA2", s("A.B.C.A2")),
                    ("FieldB", s("A.B.B")),
                    ("FieldC", s("A.B.C.C")),
                ]),
            ),
            ("FieldA", s("A.A")),
            ("FieldA2", s("")),
            ("FieldB", s("A.C.B")),
            ("FieldC", s("A.C.C")),
        ]);
        assert_eq!(
            os(&value),
            "{EmbedB={FieldA=\"A.B.C.A1\";FieldA2=\"A.B.C.A2\";FieldB=\"A.B.B\";FieldC=\"A.B.C.C\";};FieldA=\"A.A\";FieldA2=\"\";FieldB=\"A.C.B\";FieldC=\"A.C.C\";}"
        );
        assert_eq!(
            gs(&value),
            "{EmbedB={FieldA=A.B.C.A1;FieldA2=A.B.C.A2;FieldB=A.B.B;FieldC=A.B.C.C;};FieldA=A.A;FieldA2=\"\";FieldB=A.C.B;FieldC=A.C.C;}"
        );
    }

    #[test]
    fn dictionary_keys_emit_in_insertion_order() {
        let value = dict([
            ("zebra", s("1")),
            ("Alpha", s("2")),
            ("alpha", s("3")),
            ("", s("4")),
        ]);
        assert_eq!(os(&value), "{zebra=1;Alpha=2;alpha=3;\"\"=4;}");
    }

    #[test]
    fn blank_key_golden() {
        assert_eq!(os(&dict([("", s("Hello"))])), "{\"\"=Hello;}");
        assert_eq!(gs(&dict([("", s("Hello"))])), "{\"\"=Hello;}");
    }

    #[test]
    fn writer_failure_surfaces_as_io_error() {
        struct FailingWriter;
        impl io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("sink failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let result = generate(&mut FailingWriter, &s("Hello"), Format::OpenStep, "");
        assert!(matches!(result, Err(Error::Io(_))));
    }
}
