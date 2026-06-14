//! [`Encoder`] and the encode-side entry points.

use std::fmt;
use std::io::Write;

#[cfg(feature = "serde")]
use serde::Serialize;

use crate::error::Result;
use crate::format::Format;
use crate::value::Value;

/// Writes property-list documents to a writer in a chosen [`Format`].
///
/// Construction is infallible; requesting a format whose cargo feature is
/// compiled out fails at encode time with
/// [`Error::FeatureDisabled`](crate::Error::FeatureDisabled). Each successful
/// encode call writes exactly one complete document; repeated calls append
/// documents back to back with no separator.
///
/// # Examples
///
/// ```
/// use apple_plist::{Encoder, Value};
///
/// let mut out = Vec::new();
/// Encoder::new(&mut out).encode_value(&Value::from(true))?;
/// assert!(out.ends_with(b"<true/></plist>"));
/// # Ok::<(), apple_plist::Error>(())
/// ```
pub struct Encoder<W: Write> {
    #[cfg_attr(
        not(any(feature = "xml", feature = "binary", feature = "openstep")),
        expect(dead_code, reason = "no codec is compiled in to consume the writer")
    )]
    writer: W,
    format: Format,
    indent: String,
}

impl<W: Write> Encoder<W> {
    /// Creates an encoder for the default format, XML.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Encoder, Value};
    ///
    /// let mut out = Vec::new();
    /// Encoder::new(&mut out).encode_value(&Value::from("hi"))?;
    /// assert!(out.starts_with(b"<?xml"));
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    pub const fn new(writer: W) -> Self {
        Self::for_format(writer, Format::Xml)
    }

    /// Creates an encoder for an explicit format.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Encoder, Format, Value};
    ///
    /// let mut out = Vec::new();
    /// Encoder::for_format(&mut out, Format::OpenStep).encode_value(&Value::from("hi"))?;
    /// assert_eq!(out, b"hi");
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    pub const fn for_format(writer: W, format: Format) -> Self {
        Self {
            writer,
            format,
            indent: String::new(),
        }
    }

    /// Creates an encoder for the binary (`bplist00`) format.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Encoder, Value};
    ///
    /// let mut out = Vec::new();
    /// Encoder::binary(&mut out).encode_value(&Value::from(7u8))?;
    /// assert!(out.starts_with(b"bplist00"));
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    pub const fn binary(writer: W) -> Self {
        Self::for_format(writer, Format::Binary)
    }

    /// Creates an encoder that lets the library pick the format — currently
    /// binary, the automatic-format default.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Encoder, Value};
    ///
    /// let mut out = Vec::new();
    /// Encoder::automatic(&mut out).encode_value(&Value::from(7u8))?;
    /// assert!(out.starts_with(b"bplist00"));
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    pub const fn automatic(writer: W) -> Self {
        Self::for_format(writer, Format::Binary)
    }

    /// Turns on pretty-printing for the XML and text formats; the binary
    /// format ignores it.
    ///
    /// The string is written verbatim, repeated per nesting depth, and
    /// re-applied on every encode until changed. A non-empty indent also
    /// switches the text formats' key delimiter from `=` to ` = `; the empty
    /// string switches pretty-printing back off.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Encoder, Format, Value};
    ///
    /// let value = Value::from_iter([("a".to_owned(), Value::from("b"))]);
    /// let mut out = Vec::new();
    /// let mut encoder = Encoder::for_format(&mut out, Format::OpenStep);
    /// encoder.set_indent("\t");
    /// encoder.encode_value(&value)?;
    /// assert_eq!(out, b"{\n\ta = b;\n}");
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    pub fn set_indent(&mut self, indent: impl Into<String>) {
        self.indent = indent.into();
    }

    /// Serializes `value` and writes one complete document.
    ///
    /// The value is serialized into a [`Value`] tree before the format is
    /// consulted, so serialization failures win over format failures.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoRootElement`](crate::Error::NoRootElement) when the
    /// root serializes to nothing (for example `None`), any error a custom
    /// `Serialize` implementation reports, and then everything
    /// [`encode_value`](Self::encode_value) can return.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Encoder;
    ///
    /// let mut out = Vec::new();
    /// Encoder::new(&mut out).encode("Hello")?;
    /// assert!(out.ends_with(b"<string>Hello</string></plist>"));
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    #[cfg(feature = "serde")]
    pub fn encode<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        let tree = crate::value::ser::to_value(value)?;
        self.encode_value(&tree)
    }

    /// Writes one complete document holding `value`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::FeatureDisabled`](crate::Error::FeatureDisabled) when
    /// this encoder's format is behind a cargo feature that is compiled out
    /// (`xml`, `binary`, or `openstep` — the latter covers both text
    /// formats), [`Error::Io`](crate::Error::Io) when the writer fails, and
    /// [`Error::Message`](crate::Error::Message) when a binary-format
    /// container holds a `NaN` real.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Encoder, Format, Value};
    ///
    /// let mut out = Vec::new();
    /// Encoder::for_format(&mut out, Format::GnuStep).encode_value(&Value::from(3u8))?;
    /// assert_eq!(out, b"<*I3>");
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    pub fn encode_value(&mut self, value: &Value) -> Result<()> {
        match self.format {
            Format::Xml => self.encode_xml(value),
            Format::Binary => self.encode_binary(value),
            Format::OpenStep | Format::GnuStep => self.encode_text(value),
        }
    }

    #[cfg(feature = "xml")]
    fn encode_xml(&mut self, value: &Value) -> Result<()> {
        crate::xml::generator::generate(&mut self.writer, value, &self.indent)
    }

    #[cfg(not(feature = "xml"))]
    fn encode_xml(&mut self, _value: &Value) -> Result<()> {
        Err(crate::error::Error::FeatureDisabled {
            format: Format::Xml,
        })
    }

    #[cfg(feature = "binary")]
    fn encode_binary(&mut self, value: &Value) -> Result<()> {
        let document = crate::binary::generator::generate(value)?;
        self.writer.write_all(&document)?;
        Ok(())
    }

    #[cfg(not(feature = "binary"))]
    fn encode_binary(&mut self, _value: &Value) -> Result<()> {
        Err(crate::error::Error::FeatureDisabled {
            format: Format::Binary,
        })
    }

    #[cfg(feature = "openstep")]
    fn encode_text(&mut self, value: &Value) -> Result<()> {
        crate::text::generate(&mut self.writer, value, self.format, &self.indent)
    }

    #[cfg(not(feature = "openstep"))]
    fn encode_text(&mut self, _value: &Value) -> Result<()> {
        Err(crate::error::Error::FeatureDisabled {
            format: self.format,
        })
    }
}

impl<W: Write> fmt::Debug for Encoder<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoder")
            .field("format", &self.format)
            .field("indent", &self.indent)
            .finish_non_exhaustive()
    }
}

/// Serializes `value` into a new byte vector in the given format.
///
/// Equivalent to [`to_vec_indent`] with an empty indent.
///
/// # Errors
///
/// Everything [`Encoder::encode`] can return, except [`Error::Io`] — the
/// in-memory writer cannot fail.
///
/// [`Error::Io`]: crate::Error::Io
///
/// # Examples
///
/// ```
/// use apple_plist::Format;
///
/// let bytes = apple_plist::to_vec(&true, Format::OpenStep)?;
/// assert_eq!(bytes, b"1");
/// # Ok::<(), apple_plist::Error>(())
/// ```
#[cfg(feature = "serde")]
pub fn to_vec<T: Serialize>(value: &T, format: Format) -> Result<Vec<u8>> {
    to_vec_indent(value, format, "")
}

/// Serializes `value` into a new, pretty-printed byte vector.
///
/// # Errors
///
/// Everything [`Encoder::encode`] can return, except [`Error::Io`] — the
/// in-memory writer cannot fail. On error no bytes are returned.
///
/// [`Error::Io`]: crate::Error::Io
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
///
/// use apple_plist::Format;
///
/// let value = BTreeMap::from([("a", 1)]);
/// let bytes = apple_plist::to_vec_indent(&value, Format::GnuStep, "\t")?;
/// assert_eq!(bytes, b"{\n\ta = <*I1>;\n}");
/// # Ok::<(), apple_plist::Error>(())
/// ```
#[cfg(feature = "serde")]
pub fn to_vec_indent<T: Serialize>(value: &T, format: Format, indent: &str) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut encoder = Encoder::for_format(&mut buffer, format);
    encoder.set_indent(indent);
    encoder.encode(value)?;
    Ok(buffer)
}

/// Serializes `value` to `writer` in the given format.
///
/// Behaves exactly like [`Encoder::for_format`] followed by
/// [`Encoder::encode`]; indentation is encoder state, so there is no
/// `to_writer_indent`.
///
/// # Errors
///
/// Everything [`Encoder::encode`] can return, including
/// [`Error::Io`](crate::Error::Io) when `writer` fails.
///
/// # Examples
///
/// ```
/// use apple_plist::Format;
///
/// let mut out = Vec::new();
/// apple_plist::to_writer(&mut out, &7u8, Format::Binary)?;
/// assert!(out.starts_with(b"bplist00"));
/// # Ok::<(), apple_plist::Error>(())
/// ```
#[cfg(feature = "serde")]
pub fn to_writer<W: Write, T: Serialize>(writer: W, value: &T, format: Format) -> Result<()> {
    Encoder::for_format(writer, format).encode(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        Value::from_iter([
            ("name".to_owned(), Value::from("plist")),
            ("count".to_owned(), Value::from(3_u8)),
        ])
    }

    #[test]
    fn debug_elides_the_writer() {
        let encoder = Encoder::new(Vec::new());
        let rendered = format!("{encoder:?}");
        assert!(rendered.starts_with("Encoder"));
        assert!(rendered.contains("Xml"));
    }

    #[cfg(all(feature = "xml", feature = "binary", feature = "openstep"))]
    mod all_codecs {
        #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

        use super::*;
        use crate::error::Error;

        #[test]
        fn new_defaults_to_xml_and_automatic_matches_binary() {
            let mut xml = Vec::new();
            Encoder::new(&mut xml).encode_value(&sample()).unwrap();
            assert!(xml.starts_with(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));

            let mut automatic = Vec::new();
            Encoder::automatic(&mut automatic)
                .encode_value(&sample())
                .unwrap();
            let mut binary = Vec::new();
            Encoder::binary(&mut binary)
                .encode_value(&sample())
                .unwrap();
            assert_eq!(automatic, binary);
            assert!(binary.starts_with(b"bplist00"));
        }

        #[test]
        fn indent_state_persists_and_is_reapplied_every_encode() {
            let mut out = Vec::new();
            let mut encoder = Encoder::for_format(&mut out, Format::OpenStep);
            encoder.encode_value(&sample()).unwrap();
            encoder.set_indent("\t");
            encoder.encode_value(&sample()).unwrap();
            encoder.set_indent("");
            encoder.encode_value(&sample()).unwrap();
            let compact = "{name=plist;count=3;}";
            let pretty = "{\n\tname = plist;\n\tcount = 3;\n}";
            assert_eq!(out, format!("{compact}{pretty}{compact}").into_bytes());
        }

        #[test]
        fn binary_ignores_indent() {
            let mut plain = Vec::new();
            Encoder::binary(&mut plain).encode_value(&sample()).unwrap();
            let mut indented = Vec::new();
            let mut encoder = Encoder::binary(&mut indented);
            encoder.set_indent("\t");
            encoder.encode_value(&sample()).unwrap();
            assert_eq!(plain, indented);
        }

        #[test]
        fn repeated_encodes_append_complete_documents() {
            let mut out = Vec::new();
            let mut encoder = Encoder::new(&mut out);
            encoder.encode_value(&Value::from(true)).unwrap();
            encoder.encode_value(&Value::from(false)).unwrap();
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text.matches("<?xml").count(), 2);
            assert!(text.ends_with("<false/></plist>"));
        }

        #[test]
        fn failing_writers_surface_io_for_every_format() {
            struct FailingWriter;
            impl Write for FailingWriter {
                fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                    Err(std::io::Error::other("sink failure"))
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            for format in [
                Format::Xml,
                Format::Binary,
                Format::OpenStep,
                Format::GnuStep,
            ] {
                let result = Encoder::for_format(FailingWriter, format).encode_value(&sample());
                assert!(matches!(result, Err(Error::Io(_))), "{format}");
            }
        }

        #[cfg(feature = "serde")]
        #[test]
        fn to_vec_equals_to_vec_indent_with_empty_indent() {
            for format in [
                Format::Xml,
                Format::Binary,
                Format::OpenStep,
                Format::GnuStep,
            ] {
                assert_eq!(
                    to_vec(&3_u8, format).unwrap(),
                    to_vec_indent(&3_u8, format, "").unwrap(),
                    "{format}"
                );
            }
        }

        #[cfg(feature = "serde")]
        #[test]
        fn nil_roots_fail_before_any_byte_is_written() {
            struct PanickyWriter;
            impl Write for PanickyWriter {
                fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                    Err(std::io::Error::other("must not be reached"))
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let result = Encoder::new(PanickyWriter).encode(&Option::<i32>::None);
            assert!(matches!(result, Err(Error::NoRootElement)));
        }

        #[cfg(feature = "serde")]
        #[test]
        fn astral_runes_encode_in_every_format() {
            let value = "grin 😀 end";
            for format in [
                Format::Xml,
                Format::Binary,
                Format::OpenStep,
                Format::GnuStep,
            ] {
                let bytes = to_vec(&value, format).unwrap();
                assert!(!bytes.is_empty(), "{format}");
            }
            // XML and binary round-trip astral strings faithfully.
            let xml: String = crate::de::from_slice(&to_vec(&value, Format::Xml).unwrap()).unwrap();
            assert_eq!(xml, value);
            let binary: String =
                crate::de::from_slice(&to_vec(&value, Format::Binary).unwrap()).unwrap();
            assert_eq!(binary, value);
        }
    }
}
