//! The OpenStep/GNUStep text codec: one parser/generator pair serving both
//! dialects.
//!
//! The parser always starts in OpenStep mode and irreversibly upgrades to
//! GNUStep on the first `<*` or `<[` literal; the generator is constructed
//! for one dialect and differs only in its quoting table and typed-scalar
//! emission. The dialect split is runtime — the single `openstep` cargo
//! feature covers both.

use std::io::Write;

use crate::error::Result;
use crate::format::Format;
use crate::value::Value;

mod generator;
mod parser;
mod tables;

/// Parses a text plist, reporting [`Format::OpenStep`] or
/// [`Format::GnuStep`].
///
/// # Errors
///
/// Returns [`Error::Parse`](crate::Error::Parse) with format `"text"` for
/// malformed input and [`Error::MaxDepthExceeded`](crate::Error::MaxDepthExceeded)
/// past the nesting cap; text parse failures never trigger the decode
/// ladder's XML-to-text retry.
pub(crate) fn parse(data: &[u8]) -> Result<(Value, Format)> {
    parser::parse(data)
}

/// Writes `value` as a text plist in the given dialect (any non-GNUStep
/// `format` emits OpenStep).
///
/// # Errors
///
/// Returns [`Error::Io`](crate::Error::Io) when the writer fails; every
/// value serializes.
pub(crate) fn generate<W: Write>(
    writer: &mut W,
    value: &Value,
    format: Format,
    indent: &str,
) -> Result<()> {
    generator::generate(writer, value, format, indent)
}
