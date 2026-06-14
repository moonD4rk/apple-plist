//! The crate-wide error type and its [`Result`] alias.

use crate::format::Format;

/// A specialized [`Result`] for property-list operations.
///
/// [`Result`]: std::result::Result
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A boxed error carried as the optional cause of a parse failure.
type Source = Box<dyn std::error::Error + Send + Sync>;

/// Errors returned while encoding or decoding a property list.
///
/// The enum and its struct-like variants are `#[non_exhaustive]`, so new
/// variants and fields are not breaking changes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An underlying reader or writer failed.
    #[error("i/o error")]
    Io(#[from] std::io::Error),

    /// The input is not recognizable as a property list of `format`.
    ///
    /// This is the only variant that makes the decode ladder fall back from
    /// the XML parser to the text parser.
    #[error("invalid {format} property list")]
    #[non_exhaustive]
    InvalidPlist {
        /// The parser that rejected the input: `"XML"`, `"binary"`, or `"text"`.
        format: &'static str,
        /// The underlying cause, when one exists.
        source: Option<Source>,
    },

    /// The input was recognized as `format` but failed to parse. Never
    /// triggers the XML-to-text fallback.
    #[error("error parsing {format} property list")]
    #[non_exhaustive]
    Parse {
        /// The parser that failed: `"XML"`, `"binary"`, or `"text"`.
        format: &'static str,
        /// The underlying cause, when one exists.
        source: Option<Source>,
    },

    /// A value of this type cannot be represented in a property list.
    #[error("can't marshal value of type {0}")]
    UnknownType(&'static str),

    /// A property-list value cannot decode into the requested target type.
    #[error("cannot decode {found} into {expected}")]
    #[non_exhaustive]
    TypeMismatch {
        /// A description of the requested target type.
        expected: &'static str,
        /// The property-list type name that was found.
        found: &'static str,
    },

    /// Nesting exceeded [`MAX_PARSE_DEPTH`](crate::MAX_PARSE_DEPTH) while
    /// parsing — the input is too deeply nested to process safely.
    #[error("maximum nesting depth exceeded")]
    MaxDepthExceeded,

    /// A scalar literal failed to parse (integer, real, boolean, or date).
    #[error("{0}")]
    ParseScalar(String),

    /// Encoding produced no root element to write.
    #[error("no root element to encode")]
    NoRootElement,

    /// A null value reached a position where property lists cannot express it.
    #[error("null is not representable")]
    NullNotRepresentable,

    /// A free-form message, used by `serde` `Error::custom` and similar.
    #[error("{0}")]
    Message(String),

    /// The requested output format is behind a cargo feature that is not
    /// enabled in this build.
    #[error("support for the {format} format is disabled")]
    #[non_exhaustive]
    FeatureDisabled {
        /// The format whose codec is compiled out.
        format: Format,
    },
}

impl Error {
    /// Compiled only where a decode-ladder rung is feature-disabled; the
    /// enabled rungs build their retry signals with sources attached.
    #[cfg(any(test, not(feature = "binary"), not(feature = "xml")))]
    pub(crate) fn invalid(format: &'static str) -> Self {
        Self::InvalidPlist {
            format,
            source: None,
        }
    }

    #[cfg(any(test, feature = "binary", feature = "xml", feature = "openstep"))]
    pub(crate) fn parse(format: &'static str, source: impl Into<Source>) -> Self {
        Self::Parse {
            format,
            source: Some(source.into()),
        }
    }

    /// True only for [`Error::InvalidPlist`] — the decode ladder's signal to
    /// retry the buffered input with the text parser.
    pub(crate) const fn is_retry_signal(&self) -> bool {
        matches!(self, Self::InvalidPlist { .. })
    }
}

#[cfg(feature = "serde")]
impl serde::ser::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self::Message(msg.to_string())
    }
}

#[cfg(feature = "serde")]
impl serde::de::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self::Message(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_send_sync<T: Send + Sync + 'static>() {}
    const _: () = assert_send_sync::<Error>();

    #[test]
    fn display_messages_are_lowercase_unprefixed_unterminated() {
        let cases = [
            (Error::invalid("XML"), "invalid XML property list"),
            (
                Error::parse("binary", "bad trailer"),
                "error parsing binary property list",
            ),
            (
                Error::UnknownType("chan"),
                "can't marshal value of type chan",
            ),
            (
                Error::TypeMismatch {
                    expected: "u64",
                    found: "string",
                },
                "cannot decode string into u64",
            ),
            (Error::MaxDepthExceeded, "maximum nesting depth exceeded"),
            (
                Error::ParseScalar("invalid digit found in string".to_owned()),
                "invalid digit found in string",
            ),
            (Error::NoRootElement, "no root element to encode"),
            (Error::NullNotRepresentable, "null is not representable"),
            (Error::Message("boom".to_owned()), "boom"),
            (
                Error::FeatureDisabled {
                    format: Format::Binary,
                },
                "support for the Binary format is disabled",
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.to_string(), want);
        }
    }

    #[test]
    fn parse_carries_its_source() {
        let err = Error::parse("text", "unterminated string");
        let source = std::error::Error::source(&err).map(ToString::to_string);
        assert_eq!(source.as_deref(), Some("unterminated string"));
    }

    #[test]
    fn invalid_has_no_source() {
        let err = Error::invalid("XML");
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    fn retry_signal_is_invalid_plist_only() {
        assert!(Error::invalid("XML").is_retry_signal());
        assert!(!Error::parse("XML", "boom").is_retry_signal());
        assert!(!Error::MaxDepthExceeded.is_retry_signal());
        assert!(!Error::Io(std::io::Error::other("io")).is_retry_signal());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_custom_maps_to_message() {
        let ser = <Error as serde::ser::Error>::custom("ser oops");
        let de = <Error as serde::de::Error>::custom("de oops");
        assert!(matches!(ser, Error::Message(ref m) if m == "ser oops"));
        assert!(matches!(de, Error::Message(ref m) if m == "de oops"));
    }
}
