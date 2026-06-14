//! The binary (`bplist00`) property-list codec.
//!
//! [`parser::parse`] reads a fully buffered document with every input-derived
//! offset, length, and count bounds-checked; [`generator::generate`] performs
//! a two-pass preorder flatten with object uniquing and emits byte-identical
//! documents.

pub(crate) mod generator;
pub(crate) mod parser;

/// Decodes a whitespace-tolerant hex string into bytes, for embedding golden
/// fixtures in unit tests.
#[cfg(test)]
pub(crate) fn decode_hex(hex: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let mut pending = None;
    for digit in hex.chars().filter_map(|c| c.to_digit(16)) {
        match pending.take() {
            Some(high) => bytes.push(u8::try_from(high * 16 + digit).unwrap_or(0)),
            None => pending = Some(digit),
        }
    }
    bytes
}
