//! [`Decoder`], the format-detection ladder, and the decode-side entry
//! points.

use std::fmt;
use std::io::Read;

#[cfg(feature = "serde")]
use serde::de::DeserializeOwned;

use crate::error::Result;
use crate::format::Format;
use crate::value::Value;

/// Reads one property-list document from a reader, auto-detecting its
/// [`Format`].
///
/// The first decode call buffers the reader to end of input; decoding works
/// over that buffer, so the reader needs no `Seek` bound and repeated decode
/// calls re-run detection over the same bytes, returning equal values for
/// every format. Decode memory is proportional to the input size.
///
/// # Examples
///
/// ```
/// use apple_plist::{Decoder, Format};
///
/// let mut decoder = Decoder::new(&b"(1,2,3)"[..]);
/// assert_eq!(decoder.format(), None);
/// let value = decoder.decode_value()?;
/// assert_eq!(value.as_array().map(Vec::len), Some(3));
/// assert_eq!(decoder.format(), Some(Format::OpenStep));
/// # Ok::<(), apple_plist::Error>(())
/// ```
pub struct Decoder<R> {
    reader: R,
    buffer: Option<Vec<u8>>,
    format: Option<Format>,
}

impl<R: Read> Decoder<R> {
    /// Creates a decoder over `reader`; no I/O happens until the first
    /// decode call.
    pub const fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: None,
            format: None,
        }
    }

    /// The format detected by the most recent successful parse, or `None`
    /// if no parse has succeeded yet.
    ///
    /// The format is recorded before the value maps into the target type,
    /// so a failed [`decode`](Self::decode) whose document parsed still
    /// reports it.
    #[must_use]
    pub const fn format(&self) -> Option<Format> {
        self.format
    }

    /// Decodes the buffered document into a [`Value`] tree.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`](crate::Error::Io) when buffering the reader
    /// fails, and otherwise whatever the detection ladder reports:
    /// [`Error::Parse`](crate::Error::Parse) for malformed documents,
    /// [`Error::MaxDepthExceeded`](crate::Error::MaxDepthExceeded) for
    /// hostile nesting, and
    /// [`Error::InvalidPlist`](crate::Error::InvalidPlist) /
    /// [`Error::FeatureDisabled`](crate::Error::FeatureDisabled) in builds
    /// whose codec features are compiled out.
    pub fn decode_value(&mut self) -> Result<Value> {
        let (value, format) = parse_auto(self.buffered()?)?;
        self.format = Some(format);
        Ok(value)
    }

    /// Decodes the buffered document into any [`DeserializeOwned`] type.
    ///
    /// When detection reports [`Format::OpenStep`] — a format that can only
    /// store strings — the mapping coerces strings into requested integers,
    /// floats, booleans, and dates, the codec's lax mode.
    ///
    /// # Errors
    ///
    /// Everything [`decode_value`](Self::decode_value) can return, plus the
    /// mapping failures of [`from_value`](crate::from_value).
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Decoder;
    ///
    /// let document = b"<?xml version=\"1.0\"?><plist><integer>42</integer></plist>";
    /// let answer: i64 = Decoder::new(&document[..]).decode()?;
    /// assert_eq!(answer, 42);
    /// # Ok::<(), apple_plist::Error>(())
    /// ```
    #[cfg(feature = "serde")]
    pub fn decode<T: DeserializeOwned>(&mut self) -> Result<T> {
        let (value, format) = parse_auto(self.buffered()?)?;
        self.format = Some(format);
        let lax = format == Format::OpenStep;
        T::deserialize(crate::value::de::ValueDeserializer::new(value, lax))
    }

    /// Buffers the reader to end of input once; later calls reuse the
    /// buffer. A failed read keeps no partial buffer, so a subsequent call
    /// retries the reader.
    fn buffered(&mut self) -> Result<&[u8]> {
        if self.buffer.is_none() {
            let mut data = Vec::new();
            let _ = self.reader.read_to_end(&mut data)?;
            self.buffer = Some(data);
        }
        Ok(self.buffer.as_deref().unwrap_or_default())
    }
}

impl<R> fmt::Debug for Decoder<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// The detection ladder: a 6-byte `bplist` magic commits to the binary
/// parser; otherwise the XML parser runs, and only its
/// [`Error::InvalidPlist`](crate::Error::InvalidPlist) verdict falls
/// through to the text parser.
pub(crate) fn parse_auto(bytes: &[u8]) -> Result<(Value, Format)> {
    if bytes.starts_with(b"bplist") {
        return binary_rung(bytes);
    }
    let retry = match xml_rung(bytes) {
        Ok(value) => return Ok((value, Format::Xml)),
        Err(error) if error.is_retry_signal() => error,
        Err(error) => return Err(error),
    };
    text_rung(bytes, retry)
}

#[cfg(feature = "binary")]
fn binary_rung(bytes: &[u8]) -> Result<(Value, Format)> {
    crate::binary::parser::parse(bytes).map(|value| (value, Format::Binary))
}

#[cfg(not(feature = "binary"))]
fn binary_rung(_bytes: &[u8]) -> Result<(Value, Format)> {
    // The magic commits the ladder even when the codec is compiled out.
    Err(crate::error::Error::invalid("binary"))
}

#[cfg(feature = "xml")]
fn xml_rung(bytes: &[u8]) -> Result<Value> {
    crate::xml::parser::parse(bytes)
}

#[cfg(not(feature = "xml"))]
fn xml_rung(_bytes: &[u8]) -> Result<Value> {
    // A compiled-out rung behaves as if its parser returned the retry signal.
    Err(crate::error::Error::invalid("XML"))
}

#[cfg(feature = "openstep")]
fn text_rung(bytes: &[u8], _xml_failure: crate::error::Error) -> Result<(Value, Format)> {
    crate::text::parse(bytes)
}

#[cfg(not(feature = "openstep"))]
fn text_rung(_bytes: &[u8], xml_failure: crate::error::Error) -> Result<(Value, Format)> {
    if cfg!(feature = "xml") {
        Err(xml_failure)
    } else {
        Err(crate::error::Error::FeatureDisabled {
            format: Format::Xml,
        })
    }
}

/// Identifies the property-list format of `data`, or `None` when no enabled
/// codec accepts it.
///
/// Runs the full detection ladder and discards the parsed value — format
/// identification costs a complete parse, exactly like a decode. The answer
/// therefore matches what [`Decoder::format`] would report after a
/// successful decode of the same bytes, and depends on the codec features
/// enabled in this build (enabling more features only turns `None` into
/// `Some`).
///
/// # Examples
///
/// ```
/// use apple_plist::{Format, detect};
///
/// assert_eq!(detect(b"<string>hi</string>"), Some(Format::Xml));
/// assert_eq!(detect(b"(1,2,<*I3>)"), Some(Format::GnuStep));
/// assert_eq!(detect(b"bplist00"), None);
/// ```
#[must_use]
pub fn detect(data: &[u8]) -> Option<Format> {
    parse_auto(data).ok().map(|(_, format)| format)
}

/// Deserializes a property-list document from a byte slice, auto-detecting
/// its format.
///
/// Returns the value alone; use a [`Decoder`] when the detected format
/// matters.
///
/// # Errors
///
/// Everything [`Decoder::decode`] can return.
///
/// # Examples
///
/// ```
/// let answer: i64 = apple_plist::from_slice(b"<integer>42</integer>")?;
/// assert_eq!(answer, 42);
/// # Ok::<(), apple_plist::Error>(())
/// ```
#[cfg(feature = "serde")]
pub fn from_slice<T: DeserializeOwned>(data: &[u8]) -> Result<T> {
    Decoder::new(data).decode()
}

/// Deserializes a property-list document from a reader, auto-detecting its
/// format.
///
/// The reader is buffered to end of input first; decode memory is
/// proportional to the input size.
///
/// # Errors
///
/// Everything [`Decoder::decode`] can return, including
/// [`Error::Io`](crate::Error::Io) when the reader fails.
///
/// # Examples
///
/// ```
/// let answer: bool = apple_plist::from_reader(&b"<true/>"[..])?;
/// assert!(answer);
/// # Ok::<(), apple_plist::Error>(())
/// ```
#[cfg(feature = "serde")]
pub fn from_reader<R: Read, T: DeserializeOwned>(reader: R) -> Result<T> {
    Decoder::new(reader).decode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_elides_the_reader() {
        let decoder = Decoder::new(&b""[..]);
        let rendered = format!("{decoder:?}");
        assert!(rendered.starts_with("Decoder"));
        assert!(rendered.contains("None"));
    }

    #[cfg(all(feature = "xml", feature = "binary", feature = "openstep"))]
    mod full_ladder {
        #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

        use std::collections::BTreeMap;

        use super::*;
        use crate::error::Error;
        use crate::value::{Dictionary, Integer};

        /// `String.binary.plist` from the golden corpus.
        const HELLO_BPLIST: &[u8] = b"bplist00UHello\x08\
            \x00\x00\x00\x00\x00\x00\x01\x01\
            \x00\x00\x00\x00\x00\x00\x00\x01\
            \x00\x00\x00\x00\x00\x00\x00\x00\
            \x00\x00\x00\x00\x00\x00\x00\x0e";

        fn decode_bytes(data: &[u8]) -> (Result<Value>, Option<Format>) {
            let mut decoder = Decoder::new(data);
            let result = decoder.decode_value();
            (result, decoder.format())
        }

        #[test]
        fn format_detection_table_is_correct() {
            // Format detection over all seven rows.
            let (value, format) = decode_bytes(HELLO_BPLIST);
            assert_eq!(value.unwrap(), Value::String("Hello".into()));
            assert_eq!(format, Some(Format::Binary));

            let (value, format) = decode_bytes(b"<string>&lt;*I3&gt;</string>");
            assert_eq!(value.unwrap(), Value::String("<*I3>".into()));
            assert_eq!(format, Some(Format::Xml));

            let (value, format) = decode_bytes(b"bplist00");
            assert!(matches!(
                value,
                Err(Error::Parse {
                    format: "binary",
                    ..
                })
            ));
            assert_eq!(format, None);

            let (value, format) = decode_bytes(b"(1,2,3,4,5)");
            assert_eq!(
                value.unwrap(),
                Value::Array(
                    ["1", "2", "3", "4", "5"]
                        .map(|s| Value::String(s.into()))
                        .to_vec()
                )
            );
            assert_eq!(format, Some(Format::OpenStep));

            let (value, format) = decode_bytes(b"<abab>");
            assert_eq!(value.unwrap(), Value::Data(vec![0xAB, 0xAB]));
            assert_eq!(format, Some(Format::OpenStep));

            let (value, format) = decode_bytes(b"(1,2,<*I3>)");
            assert_eq!(
                value.unwrap(),
                Value::Array(vec![
                    Value::String("1".into()),
                    Value::String("2".into()),
                    Value::Integer(Integer::Signed(3)),
                ])
            );
            assert_eq!(format, Some(Format::GnuStep));

            let (value, format) = decode_bytes(b"\x00");
            assert!(matches!(value, Err(Error::Parse { format: "text", .. })));
            assert_eq!(format, None);
        }

        #[test]
        fn detect_agrees_with_the_ladder() {
            assert_eq!(detect(HELLO_BPLIST), Some(Format::Binary));
            assert_eq!(detect(b"<string>&lt;*I3&gt;</string>"), Some(Format::Xml));
            assert_eq!(detect(b"bplist00"), None);
            assert_eq!(detect(b"(1,2,3,4,5)"), Some(Format::OpenStep));
            assert_eq!(detect(b"<abab>"), Some(Format::OpenStep));
            assert_eq!(detect(b"(1,2,<*I3>)"), Some(Format::GnuStep));
            assert_eq!(detect(b"\x00"), None);
            assert_eq!(detect(b""), Some(Format::OpenStep));
        }

        #[test]
        fn empty_whitespace_and_comment_only_input_is_an_empty_dictionary() {
            for input in [&b""[..], b" \n\t", b"// hi", b"/* hi */"] {
                let (value, format) = decode_bytes(input);
                assert_eq!(value.unwrap(), Value::Dictionary(Dictionary::new()));
                assert_eq!(format, Some(Format::OpenStep));
            }
        }

        #[test]
        fn short_non_magic_prefixes_never_sniff_as_binary() {
            let (value, format) = decode_bytes(b"bplis");
            assert_eq!(value.unwrap(), Value::String("bplis".into()));
            assert_eq!(format, Some(Format::OpenStep));

            // Exactly the magic commits and fails inside the binary parser.
            let (value, format) = decode_bytes(b"bplist");
            assert!(matches!(
                value,
                Err(Error::Parse {
                    format: "binary",
                    ..
                })
            ));
            assert_eq!(format, None);

            // The magic commits even for would-be OpenStep documents.
            let (value, _) = decode_bytes(b"bplistish = x;");
            assert!(matches!(
                value,
                Err(Error::Parse {
                    format: "binary",
                    ..
                })
            ));
        }

        #[test]
        fn xml_hard_errors_do_not_retry_as_text() {
            let (value, format) = decode_bytes(b"<plist/>");
            assert!(matches!(value, Err(Error::Parse { format: "XML", .. })));
            assert_eq!(format, None);

            let (value, _) = decode_bytes(b"<plist>");
            assert!(matches!(value, Err(Error::Parse { format: "XML", .. })));
        }

        #[test]
        fn xml_depth_overrun_is_fatal_without_text_retry() {
            let mut doc = Vec::new();
            for _ in 0..200 {
                doc.extend_from_slice(b"<array>");
            }
            let (value, format) = decode_bytes(&doc);
            assert!(matches!(value, Err(Error::MaxDepthExceeded)));
            assert_eq!(format, None);
        }

        #[test]
        fn when_xml_retries_and_text_fails_the_text_error_surfaces() {
            let (value, format) = decode_bytes(b"{ a = ");
            assert!(matches!(value, Err(Error::Parse { format: "text", .. })));
            assert_eq!(format, None);
        }

        #[test]
        fn bom_matrix_follows_the_ladder() {
            let (value, format) = decode_bytes(b"\xEF\xBB\xBF<string>x</string>");
            assert_eq!(value.unwrap(), Value::String("x".into()));
            assert_eq!(format, Some(Format::Xml));

            let (value, format) = decode_bytes(b"\xEF\xBB\xBF{a=b;}");
            assert_eq!(
                value.unwrap(),
                Value::Dictionary(Dictionary::from([(
                    "a".to_owned(),
                    Value::String("b".into()),
                )]))
            );
            assert_eq!(format, Some(Format::OpenStep));

            let mut bom_bplist = b"\xEF\xBB\xBF".to_vec();
            bom_bplist.extend_from_slice(HELLO_BPLIST);
            assert_ne!(detect(&bom_bplist), Some(Format::Binary));
        }

        #[test]
        fn repeated_decodes_are_idempotent_for_every_format() {
            let documents: [&[u8]; 4] =
                [HELLO_BPLIST, b"{a=b;}", b"(<*I1>)", b"<string>x</string>"];
            for document in documents {
                let mut decoder = Decoder::new(document);
                let first = decoder.decode_value().unwrap();
                let first_format = decoder.format();
                let second = decoder.decode_value().unwrap();
                assert_eq!(first, second);
                assert_eq!(decoder.format(), first_format);
            }
        }

        #[test]
        fn parse_failures_leave_the_previous_format_in_place() {
            let mut decoder = Decoder::new(&b"bplist00"[..]);
            assert!(decoder.decode_value().is_err());
            assert_eq!(decoder.format(), None);
            assert!(decoder.decode_value().is_err());
            assert_eq!(decoder.format(), None);
        }

        #[test]
        fn io_failures_surface_and_a_later_call_retries_the_reader() {
            struct FlakyReader {
                attempts: usize,
            }
            impl Read for FlakyReader {
                fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                    self.attempts += 1;
                    if self.attempts == 1 {
                        Err(std::io::Error::other("transient"))
                    } else {
                        Ok(0)
                    }
                }
            }
            let mut decoder = Decoder::new(FlakyReader { attempts: 0 });
            assert!(matches!(decoder.decode_value(), Err(Error::Io(_))));
            assert_eq!(decoder.format(), None);
            // The retry reaches end of input: an empty OpenStep dictionary.
            assert_eq!(
                decoder.decode_value().unwrap(),
                Value::Dictionary(Dictionary::new())
            );
            assert_eq!(decoder.format(), Some(Format::OpenStep));
        }

        #[cfg(feature = "serde")]
        mod with_serde {
            use serde::Deserialize;

            use super::*;
            use crate::date::Date;

            #[test]
            fn format_is_recorded_before_the_mapping_fails() {
                let mut decoder = Decoder::new(&b"<string>abc</string>"[..]);
                let result: Result<i64> = decoder.decode();
                assert!(result.is_err());
                assert_eq!(decoder.format(), Some(Format::Xml));
            }

            #[test]
            fn lax_decode_coerces_strings_for_openstep_only() {
                // Lax decode through the public ladder.
                #[derive(Deserialize, Debug, PartialEq)]
                struct LaxTestData {
                    #[serde(rename = "I64")]
                    signed: i64,
                    #[serde(rename = "U64")]
                    unsigned: u64,
                    #[serde(rename = "F64")]
                    float: f64,
                    #[serde(rename = "B")]
                    flag: bool,
                    #[serde(rename = "D")]
                    date: Date,
                }
                let document = br#"{B=1;D="2013-11-27 00:34:00 +0000";I64=1;F64="3.0";U64=2;}"#;
                let mut decoder = Decoder::new(&document[..]);
                let parsed: LaxTestData = decoder.decode().unwrap();
                assert_eq!(decoder.format(), Some(Format::OpenStep));
                assert_eq!(
                    parsed,
                    LaxTestData {
                        signed: 1,
                        unsigned: 2,
                        float: 3.0,
                        flag: true,
                        date: Date::parse_text_layout("2013-11-27 00:34:00 +0000").unwrap(),
                    }
                );

                // The same coercion is rejected for strict (XML) documents.
                let strict: Result<i64> = from_slice(b"<string>1</string>");
                assert!(strict.is_err());

                // Lax coercion failures still error.
                let bad: Result<i64> = from_slice(b"abc");
                assert!(bad.is_err());
            }

            #[test]
            fn decode_value_and_decode_into_value_agree() {
                let documents: [&[u8]; 4] = [
                    b"<array><integer>1</integer></array>",
                    b"{a=b;}",
                    b"(<*R1.5>)",
                    HELLO_BPLIST,
                ];
                for document in documents {
                    let direct = Decoder::new(document).decode_value().unwrap();
                    let mapped: Value = Decoder::new(document).decode().unwrap();
                    assert_eq!(direct, mapped);
                }
            }

            #[test]
            fn chunked_readers_still_detect_binary() {
                // Short-read format detection.
                struct ChunkedReader<'a> {
                    data: &'a [u8],
                    chunk: usize,
                }
                impl Read for ChunkedReader<'_> {
                    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                        let take = self.chunk.min(self.data.len()).min(buf.len());
                        let (head, tail) = self.data.split_at(take);
                        buf.get_mut(..take)
                            .map(|slot| slot.copy_from_slice(head))
                            .ok_or_else(|| std::io::Error::other("buffer too small"))?;
                        self.data = tail;
                        Ok(take)
                    }
                }
                let document =
                    crate::ser::to_vec(&BTreeMap::from([("a", "b"), ("c", "d")]), Format::Binary)
                        .unwrap();
                for chunk in [1, 2, 3, 5] {
                    let mut decoder = Decoder::new(ChunkedReader {
                        data: &document,
                        chunk,
                    });
                    let map: BTreeMap<String, String> = decoder.decode().unwrap();
                    assert_eq!(decoder.format(), Some(Format::Binary), "chunk {chunk}");
                    assert_eq!(map.len(), 2);
                }
            }
        }
    }
}
