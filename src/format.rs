//! The four property-list serialization formats.

use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// A property-list serialization format.
///
/// One constant per wire format. Pass one to an
/// encoder to choose the wire format explicitly; the decoder reports the
/// variant it detected. The enum is deliberately exhaustive: a fifth format
/// would be a semver-major event, and downstream `match` needs no wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Apple XML property list (`<?xml ...?>` + `<!DOCTYPE plist ...>`).
    Xml,
    /// Apple binary property list (`bplist00`).
    Binary,
    /// OpenStep ASCII property list.
    OpenStep,
    /// GNUStep ASCII property list — OpenStep with typed `<*...>` literals.
    GnuStep,
}

impl Format {
    const ALL: [Self; 4] = [Self::Xml, Self::Binary, Self::OpenStep, Self::GnuStep];

    /// The human-readable name of this format.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Format;
    ///
    /// assert_eq!(Format::GnuStep.name(), "GNUStep");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Xml => "XML",
            Self::Binary => "Binary",
            Self::OpenStep => "OpenStep",
            Self::GnuStep => "GNUStep",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Parses a format from its name, case-insensitively.
///
/// Accepts exactly the four [`Format::name`] spellings in any ASCII case, so
/// parsing round-trips with [`Display`](fmt::Display).
///
/// # Errors
///
/// Returns [`Error::Message`] when the input matches none of the four names.
///
/// # Examples
///
/// ```
/// use apple_plist::Format;
///
/// assert_eq!("xml".parse::<Format>().ok(), Some(Format::Xml));
/// assert_eq!("GNUSTEP".parse::<Format>().ok(), Some(Format::GnuStep));
/// assert!("plist".parse::<Format>().is_err());
/// ```
impl FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|format| s.eq_ignore_ascii_case(format.name()))
            .ok_or_else(|| Error::Message(format!("unknown format name: {s}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matches_canonical_spellings() {
        assert_eq!(Format::Xml.name(), "XML");
        assert_eq!(Format::Binary.name(), "Binary");
        assert_eq!(Format::OpenStep.name(), "OpenStep");
        assert_eq!(Format::GnuStep.name(), "GNUStep");
    }

    #[test]
    fn from_str_round_trips_display_case_insensitively() {
        for format in Format::ALL {
            assert_eq!(format.to_string().parse::<Format>().ok(), Some(format));
            assert_eq!(
                format.name().to_lowercase().parse::<Format>().ok(),
                Some(format)
            );
            assert_eq!(
                format.name().to_uppercase().parse::<Format>().ok(),
                Some(format)
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown_names() {
        for bad in ["", "plist", "XM L", " xml", "xml ", "GNU Step"] {
            let err = bad.parse::<Format>();
            assert!(
                matches!(err, Err(Error::Message(ref m)) if m.starts_with("unknown format name"))
            );
        }
    }
}
