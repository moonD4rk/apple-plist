//! The XML property-list codec: parser and generator.
//!
//! quick-xml supplies the tokenizer only; entity expansion, character-range
//! validation, `]]>` rejection, and `<?xml?>` version/encoding checks are
//! hand-rolled to enforce strict XML well-formedness.

pub(crate) mod generator;
pub(crate) mod parser;

/// Tests the XML 1.0 `Char` production, minus the surrogate gap Rust's
/// `char` already excludes.
pub(crate) const fn in_character_range(c: char) -> bool {
    matches!(
        c,
        '\u{09}' | '\u{0A}' | '\u{0D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}
