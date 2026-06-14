//! The XML plist generator.

use std::io::Write;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::error::Result;
use crate::scalar;
use crate::value::{Integer, Value};
use crate::xml::in_character_range;

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

/// Writes `value` as a complete XML plist document: fixed header, the
/// `<plist version="1.0">` wrapper, no trailing newline.
///
/// A non-empty `indent` breaks every element onto its own line, repeated per
/// depth, with the first newline suppressed so the wrapper hugs the DOCTYPE.
///
/// # Errors
///
/// Returns [`Error::Io`](crate::Error::Io) when the writer fails; every
/// value serializes.
pub(crate) fn generate<W: Write>(writer: &mut W, value: &Value, indent: &str) -> Result<()> {
    let mut generator = Generator {
        writer,
        indent,
        depth: 0,
        put_newline: false,
    };
    generator.write_str(HEADER)?;
    generator.open_tag("plist version=\"1.0\"")?;
    generator.write_value(value)?;
    generator.close_tag("plist")
}

struct Generator<'a, W> {
    writer: &'a mut W,
    indent: &'a str,
    depth: usize,
    put_newline: bool,
}

impl<W: Write> Generator<'_, W> {
    fn write_str(&mut self, s: &str) -> Result<()> {
        self.writer.write_all(s.as_bytes())?;
        Ok(())
    }

    fn write_indent(&mut self, delta: i8) -> Result<()> {
        let indent = self.indent;
        if indent.is_empty() {
            return Ok(());
        }
        if delta < 0 {
            self.depth = self.depth.saturating_sub(1);
        }
        if self.put_newline {
            self.write_str("\n")?;
        } else {
            self.put_newline = true;
        }
        for _ in 0..self.depth {
            self.write_str(indent)?;
        }
        if delta > 0 {
            self.depth += 1;
        }
        Ok(())
    }

    fn open_tag(&mut self, name: &str) -> Result<()> {
        self.write_indent(1)?;
        self.write_str("<")?;
        self.write_str(name)?;
        self.write_str(">")
    }

    fn close_tag(&mut self, name: &str) -> Result<()> {
        self.write_indent(-1)?;
        self.write_str("</")?;
        self.write_str(name)?;
        self.write_str(">")
    }

    /// One leaf element; an empty body self-closes (`<string/>`).
    fn element(&mut self, name: &str, body: &str) -> Result<()> {
        self.write_indent(0)?;
        self.write_str("<")?;
        self.write_str(name)?;
        if body.is_empty() {
            return self.write_str("/>");
        }
        self.write_str(">")?;
        self.write_escaped(body)?;
        self.write_str("</")?;
        self.write_str(name)?;
        self.write_str(">")
    }

    /// Escapes text exactly: five markup escapes, numeric whitespace
    /// escapes, U+FFFD for out-of-range runes, all else unchanged.
    fn write_escaped(&mut self, body: &str) -> Result<()> {
        for c in body.chars() {
            match c {
                '"' => self.write_str("&#34;")?,
                '\'' => self.write_str("&#39;")?,
                '&' => self.write_str("&amp;")?,
                '<' => self.write_str("&lt;")?,
                '>' => self.write_str("&gt;")?,
                '\t' => self.write_str("&#x9;")?,
                '\n' => self.write_str("&#xA;")?,
                '\r' => self.write_str("&#xD;")?,
                c if !in_character_range(c) => self.write_str("\u{FFFD}")?,
                c => {
                    let mut buf = [0_u8; 4];
                    self.write_str(c.encode_utf8(&mut buf))?;
                }
            }
        }
        Ok(())
    }

    fn write_value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Dictionary(entries) => {
                self.open_tag("dict")?;
                for (key, entry) in entries {
                    self.element("key", key)?;
                    self.write_value(entry)?;
                }
                self.close_tag("dict")
            }
            Value::Array(values) => {
                self.open_tag("array")?;
                for entry in values {
                    self.write_value(entry)?;
                }
                self.close_tag("array")
            }
            Value::String(s) => self.element("string", s),
            Value::Integer(Integer::Signed(signed)) => self.element("integer", &signed.to_string()),
            Value::Integer(Integer::Unsigned(unsigned)) => {
                self.element("integer", &unsigned.to_string())
            }
            Value::Real(real) => self.element("real", &format_xml_float(real.value())),
            Value::Boolean(true) => self.element("true", ""),
            Value::Boolean(false) => self.element("false", ""),
            Value::Uid(uid) => {
                self.open_tag("dict")?;
                self.element("key", "CF$UID")?;
                self.element("integer", &uid.get().to_string())?;
                self.close_tag("dict")
            }
            Value::Data(data) => self.element("data", &STANDARD.encode(data)),
            Value::Date(date) => self.element("date", &date.format_rfc3339()),
        }
    }
}

/// Formats a real for XML: lowercase specials, else shortest `'g'`-style form.
fn format_xml_float(value: f64) -> String {
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "inf"
        } else {
            "-inf"
        }
        .to_owned();
    }
    if value.is_nan() {
        return "nan".to_owned();
    }
    scalar::format_f64(value)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

    use super::*;
    use crate::date::Date;
    use crate::uid::Uid;
    use crate::value::{Dictionary, Real};

    fn render(value: &Value, indent: &str) -> String {
        let mut out = Vec::new();
        generate(&mut out, value, indent).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn body(value: &Value) -> String {
        let rendered = render(value, "");
        rendered
            .strip_prefix(HEADER)
            .and_then(|rest| rest.strip_prefix("<plist version=\"1.0\">"))
            .and_then(|rest| rest.strip_suffix("</plist>"))
            .unwrap()
            .to_owned()
    }

    #[test]
    fn compact_document_matches_the_golden_bytes() {
        let value = Value::String("Hello".into());
        assert_eq!(
            render(&value, ""),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><string>Hello</string></plist>"
        );
    }

    #[test]
    fn indented_document_suppresses_the_first_newline() {
        let value = Value::Dictionary(Dictionary::from([
            ("Name".to_owned(), Value::String("Dustin".into())),
            (
                "Lines".to_owned(),
                Value::Array(vec![Value::String("a".into()), Value::String("b".into())]),
            ),
        ]));
        let expected = format!(
            "{HEADER}<plist version=\"1.0\">\n\t<dict>\n\t\t<key>Name</key>\n\t\t<string>Dustin</string>\n\t\t<key>Lines</key>\n\t\t<array>\n\t\t\t<string>a</string>\n\t\t\t<string>b</string>\n\t\t</array>\n\t</dict>\n</plist>"
        );
        assert_eq!(render(&value, "\t"), expected);
    }

    #[test]
    fn empty_forms_self_close_except_containers() {
        assert_eq!(body(&Value::String(String::new())), "<string/>");
        assert_eq!(body(&Value::Data(vec![])), "<data/>");
        assert_eq!(body(&Value::Boolean(true)), "<true/>");
        assert_eq!(body(&Value::Boolean(false)), "<false/>");
        assert_eq!(body(&Value::Dictionary(Dictionary::new())), "<dict></dict>");
        assert_eq!(body(&Value::Array(vec![])), "<array></array>");
        assert_eq!(
            body(&Value::Dictionary(Dictionary::from([(
                String::new(),
                Value::String("Hello".into()),
            )]))),
            "<dict><key/><string>Hello</string></dict>"
        );
    }

    #[test]
    fn escaping_follows_xml_rules() {
        assert_eq!(
            body(&Value::String("\"'&<>\t\n\r".into())),
            "<string>&#34;&#39;&amp;&lt;&gt;&#x9;&#xA;&#xD;</string>"
        );
        assert_eq!(
            body(&Value::String("\u{0}\u{FFFE}\u{FFFF}".into())),
            "<string>\u{FFFD}\u{FFFD}\u{FFFD}</string>"
        );
        assert_eq!(
            body(&Value::String("Hello, 世界 😀 \u{7f}".into())),
            "<string>Hello, 世界 😀 \u{7f}</string>"
        );
        assert_eq!(body(&Value::String("'".into())), "<string>&#39;</string>");
    }

    #[test]
    fn reals_format_shortest_round_trip() {
        let cases: &[(f64, &str)] = &[
            (1.0, "1"),
            (32.0, "32"),
            (-0.0, "-0"),
            (1e6, "1e+06"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (std::f64::consts::PI, "3.141592653589793"),
            (f64::from(f32::MAX), "3.4028234663852886e+38"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
        ];
        for &(input, expected) in cases {
            assert_eq!(
                body(&Value::Real(Real::from(input))),
                format!("<real>{expected}</real>"),
                "{input}"
            );
        }
    }

    #[test]
    fn integers_emit_plain_decimal() {
        assert_eq!(
            body(&Value::Integer(Integer::Signed(i64::MIN))),
            "<integer>-9223372036854775808</integer>"
        );
        assert_eq!(
            body(&Value::Integer(Integer::Unsigned(
                16_045_690_985_305_262_846
            ))),
            "<integer>16045690985305262846</integer>"
        );
        assert_eq!(
            body(&Value::Integer(Integer::Signed(10))),
            "<integer>10</integer>"
        );
    }

    #[test]
    fn dates_emit_utc_z_with_subseconds_dropped() {
        let date = Date::parse_rfc3339("2013-11-27T00:34:00.75Z").unwrap();
        assert_eq!(
            body(&Value::Date(date)),
            "<date>2013-11-27T00:34:00Z</date>"
        );
    }

    #[test]
    fn uids_lower_to_their_dictionary_form() {
        assert_eq!(
            body(&Value::Uid(Uid::from(1024))),
            "<dict><key>CF$UID</key><integer>1024</integer></dict>"
        );
        assert_eq!(
            body(&Value::Array(vec![Value::Uid(Uid::from(
                1_099_511_627_775
            ))])),
            "<array><dict><key>CF$UID</key><integer>1099511627775</integer></dict></array>"
        );
    }

    #[test]
    fn data_emits_one_padded_unwrapped_run() {
        assert_eq!(
            body(&Value::Data(b"hello".to_vec())),
            "<data>aGVsbG8=</data>"
        );
        let long = vec![0xAB_u8; 1000];
        let rendered = body(&Value::Data(long));
        assert!(!rendered.contains('\n'));
        assert!(rendered.ends_with("</data>"));
    }

    #[test]
    fn dictionary_keys_emit_in_insertion_order_and_escape() {
        let value = Value::Dictionary(Dictionary::from([
            ("b".to_owned(), Value::Integer(Integer::Unsigned(2))),
            ("a<".to_owned(), Value::Integer(Integer::Unsigned(1))),
        ]));
        assert_eq!(
            body(&value),
            "<dict><key>b</key><integer>2</integer><key>a&lt;</key><integer>1</integer></dict>"
        );
    }

    #[test]
    fn write_failures_surface_as_io_errors() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("nope"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let result = generate(&mut FailingWriter, &Value::Boolean(true), "");
        assert!(matches!(result, Err(crate::Error::Io(_))));
    }

    #[test]
    fn round_trips_through_the_parser() {
        let value = Value::Dictionary(Dictionary::from([
            (
                "strings".to_owned(),
                Value::Array(vec![
                    Value::String("grin 😀 end".into()),
                    Value::String(String::new()),
                ]),
            ),
            ("count".to_owned(), Value::Integer(Integer::Unsigned(42))),
            (
                "pi".to_owned(),
                Value::Real(Real::from(std::f64::consts::PI)),
            ),
            ("yes".to_owned(), Value::Boolean(true)),
            ("blob".to_owned(), Value::Data(vec![1, 2, 3, 4])),
            (
                "when".to_owned(),
                Value::Date(Date::parse_rfc3339("2013-11-27T00:34:00Z").unwrap()),
            ),
            ("ref".to_owned(), Value::Uid(Uid::from(7))),
        ]));
        for indent in ["", "\t", "  "] {
            let mut out = Vec::new();
            generate(&mut out, &value, indent).unwrap();
            let reparsed = crate::xml::parser::parse(&out).unwrap();
            assert_eq!(reparsed, value, "indent {indent:?}");
        }
    }
}
