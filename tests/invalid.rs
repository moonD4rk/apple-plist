//! Negative-table suite: every malformed property list rejected through the
//! public surface returns an `Err` and never panics.
//!
//! Covers the binary negative table (`InvalidBplists` + `overflowBplist`), the
//! XML negative table (`InvalidXMLPlists`), and the text negative table
//! (`InvalidTextPlists`). Each fixture is fed to `from_slice::<Value>` — the
//! full detection ladder — inside `std::panic::catch_unwind` so a panic is a
//! test failure, not an abort (RFC 0005 §2).
//!
//! The exhaustive unit-level negative tables (the direct-parser passes,
//! including the `bplist`-prefix-failing "Bad magic" entry) live in
//! `src/binary/parser.rs` and `src/xml/parser.rs`; this file is the
//! public-surface pass.

#![expect(
    clippy::panic,
    reason = "the panic-absence harness reports a recovered panic as a test failure"
)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use apple_plist::{Value, from_slice};

/// Asserts that `data` is rejected (any `Err`) without panicking. The decode
/// runs inside the suite's only `catch_unwind` (RFC 0005 §2), so a panic in
/// the parser surfaces as a clean test failure rather than an abort.
fn assert_rejected(label: &str, data: &[u8]) {
    let outcome = catch_unwind(AssertUnwindSafe(|| from_slice::<Value>(data)));
    let Ok(result) = outcome else {
        panic!("{label}: from_slice panicked (must return Err, never panic)");
    };
    assert!(
        result.is_err(),
        "{label}: expected Err, decoded successfully as {:?}",
        result.ok(),
    );
}

// -- Binary (InvalidBplists) -------------------------------------------------

/// All 28 `InvalidBplists` byte blobs, fixtures `invalid-b-00..27`. Index 1 is
/// the "Bad magic" entry (first byte `x`): the ladder's `bplist` sniff fails,
/// so it never reaches the binary parser — the verdict below is the ladder's
/// incidental text-rung outcome, asserted here and documented as such. The
/// direct-parser pass over every entry (the binary-parser contract) lives in
/// `src/binary/parser.rs::all_twenty_eight_invalid_documents_error`.
const INVALID_BPLISTS: &[&[u8]] = &[
    include_bytes!("fixtures/invalid/invalid-b-00.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-01.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-02.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-03.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-04.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-05.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-06.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-07.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-08.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-09.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-10.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-11.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-12.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-13.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-14.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-15.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-16.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-17.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-18.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-19.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-20.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-21.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-22.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-23.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-24.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-25.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-26.binary.plist"),
    include_bytes!("fixtures/invalid/invalid-b-27.binary.plist"),
];

#[test]
fn invalid_bplist_table_is_rejected_through_the_ladder() {
    assert_eq!(INVALID_BPLISTS.len(), 28);
    for (index, data) in INVALID_BPLISTS.iter().enumerate() {
        // index 1 ("Bad magic") does not sniff as binary: the ladder routes it
        // through XML then text, which also reject it. Asserting the ladder
        // verdict, not the binary-parser verdict (that is the unit test's job).
        assert_rejected(&format!("invalid-b-{index:02}"), data);
    }
}

// -- Crafted count overflow (overflowBplist) ---------------------------------

/// `overflowBplist(tag)`: magic + `tag 0x13` + an 8-byte extended count of
/// `2^64-1` + a 1-byte offset table + a trailer with `OffsetTableOffset = 0x12`.
/// The keystone count guard (`cnt > OffsetTableOffset`) must reject before any
/// `count * K` / `start + count` arithmetic, so no overflow trap fires even
/// with `overflow-checks = true` on the test profile.
fn overflow_bplist(tag: u8) -> Vec<u8> {
    let mut doc = b"bplist00".to_vec();
    doc.push(tag);
    doc.push(0x13);
    doc.extend_from_slice(&u64::MAX.to_be_bytes());
    doc.push(0x08);
    doc.extend_from_slice(&[0x00; 5]); // trailer: unused[5]
    doc.push(0x00); // sort version
    doc.push(0x01); // offset int size
    doc.push(0x01); // object ref size
    doc.extend_from_slice(&1u64.to_be_bytes()); // num objects
    doc.extend_from_slice(&0u64.to_be_bytes()); // top object
    doc.extend_from_slice(&0x12u64.to_be_bytes()); // offset table offset
    doc
}

#[test]
fn crafted_count_overflow_returns_error_not_crash() {
    for (name, tag) in [
        ("dataTag", 0x4Fu8),
        ("asciiTag", 0x5F),
        ("utf16Tag", 0x6F),
        ("arrayTag", 0xAF),
        ("dictTag", 0xDF),
    ] {
        let doc = overflow_bplist(tag);
        assert_rejected(&format!("overflow {name}"), &doc);
    }
}

// -- XML (InvalidXMLPlists) --------------------------------------------------

/// All 21 `InvalidXMLPlists` inputs, by `Name`. Index 20 ("binary plist magic
/// as XML", input `bplist00`) commits the ladder to the binary parser via the
/// sniff and fails there; every other entry fails the XML rung and also the
/// text fallback. The direct-parser pass lives in `src/xml/parser.rs`.
const INVALID_XML: &[(&str, &[u8])] = &[
    (
        "hex integer with no digits",
        include_bytes!("fixtures/invalid/hex integer with no digits.xml.plist"),
    ),
    (
        "unknown element doct",
        include_bytes!("fixtures/invalid/unknown element doct.xml.plist"),
    ),
    (
        "dict with string instead of key",
        include_bytes!("fixtures/invalid/dict with string instead of key.xml.plist"),
    ),
    (
        "dict with key but no value",
        include_bytes!("fixtures/invalid/dict with key but no value.xml.plist"),
    ),
    (
        "integer with non-numeric value",
        include_bytes!("fixtures/invalid/integer with non-numeric value.xml.plist"),
    ),
    (
        "empty integer",
        include_bytes!("fixtures/invalid/empty integer.xml.plist"),
    ),
    (
        "real with non-numeric value",
        include_bytes!("fixtures/invalid/real with non-numeric value.xml.plist"),
    ),
    (
        "data with invalid base64",
        include_bytes!("fixtures/invalid/data with invalid base64.xml.plist"),
    ),
    (
        "date with invalid format",
        include_bytes!("fixtures/invalid/date with invalid format.xml.plist"),
    ),
    (
        "unclosed integer tag",
        include_bytes!("fixtures/invalid/unclosed integer tag.xml.plist"),
    ),
    (
        "unclosed real tag",
        include_bytes!("fixtures/invalid/unclosed real tag.xml.plist"),
    ),
    (
        "unclosed string tag",
        include_bytes!("fixtures/invalid/unclosed string tag.xml.plist"),
    ),
    (
        "unclosed dict tag",
        include_bytes!("fixtures/invalid/unclosed dict tag.xml.plist"),
    ),
    (
        "unclosed key tag",
        include_bytes!("fixtures/invalid/unclosed key tag.xml.plist"),
    ),
    (
        "truncated plist open tag",
        include_bytes!("fixtures/invalid/truncated plist open tag.xml.plist"),
    ),
    (
        "truncated data tag",
        include_bytes!("fixtures/invalid/truncated data tag.xml.plist"),
    ),
    (
        "truncated date tag",
        include_bytes!("fixtures/invalid/truncated date tag.xml.plist"),
    ),
    (
        "truncated array tag",
        include_bytes!("fixtures/invalid/truncated array tag.xml.plist"),
    ),
    (
        "self-closing empty plist",
        include_bytes!("fixtures/invalid/self-closing empty plist.xml.plist"),
    ),
    (
        "truncated XML",
        include_bytes!("fixtures/invalid/truncated XML.xml.plist"),
    ),
    (
        "binary plist magic as XML",
        include_bytes!("fixtures/invalid/binary plist magic as XML.xml.plist"),
    ),
];

#[test]
fn invalid_xml_table_is_rejected_through_the_ladder() {
    assert_eq!(INVALID_XML.len(), 21);
    for (name, data) in INVALID_XML {
        assert_rejected(name, data);
    }
}

// -- Text (InvalidTextPlists) ------------------------------------------------

/// All 38 `InvalidTextPlists` inputs, by `Name`. The "Invalid GNUStep base64
/// data" entry (`<[3]>`) carries a `TODO: this is actually valid`; the current
/// verdict is `Err` ("3" is not a multiple of 4 for the base64 decoder) — kept
/// as-is, not "fixed". `(missing >)` was sanitized to `(missing _)` in the
/// fixture filename by the filename sanitizer.
const INVALID_TEXT: &[(&str, &[u8])] = &[
    (
        "Truncated array",
        include_bytes!("fixtures/invalid/Truncated array.openstep.plist"),
    ),
    (
        "Truncated dictionary",
        include_bytes!("fixtures/invalid/Truncated dictionary.openstep.plist"),
    ),
    (
        "Truncated dictionary 2",
        include_bytes!("fixtures/invalid/Truncated dictionary 2.openstep.plist"),
    ),
    (
        "Unclosed nested array",
        include_bytes!("fixtures/invalid/Unclosed nested array.openstep.plist"),
    ),
    (
        "Unclosed dictionary",
        include_bytes!("fixtures/invalid/Unclosed dictionary.openstep.plist"),
    ),
    (
        "Broken GNUStep data",
        include_bytes!("fixtures/invalid/Broken GNUStep data.gnustep.plist"),
    ),
    (
        "Truncated nested array",
        include_bytes!("fixtures/invalid/Truncated nested array.openstep.plist"),
    ),
    (
        "Truncated dictionary with comment-like",
        include_bytes!("fixtures/invalid/Truncated dictionary with comment-like.openstep.plist"),
    ),
    (
        "Truncated array with comment-like",
        include_bytes!("fixtures/invalid/Truncated array with comment-like.openstep.plist"),
    ),
    (
        "Truncated array with empty data",
        include_bytes!("fixtures/invalid/Truncated array with empty data.openstep.plist"),
    ),
    (
        "Bad Extended Character",
        include_bytes!("fixtures/invalid/Bad Extended Character.openstep.plist"),
    ),
    (
        "Missing Equals in Dictionary",
        include_bytes!("fixtures/invalid/Missing Equals in Dictionary.openstep.plist"),
    ),
    (
        "Missing Semicolon in Dictionary",
        include_bytes!("fixtures/invalid/Missing Semicolon in Dictionary.openstep.plist"),
    ),
    (
        "Invalid GNUStep type",
        include_bytes!("fixtures/invalid/Invalid GNUStep type.gnustep.plist"),
    ),
    (
        "Invalid GNUStep int",
        include_bytes!("fixtures/invalid/Invalid GNUStep int.gnustep.plist"),
    ),
    (
        "Invalid GNUStep date",
        include_bytes!("fixtures/invalid/Invalid GNUStep date.gnustep.plist"),
    ),
    (
        "Truncated GNUStep value",
        include_bytes!("fixtures/invalid/Truncated GNUStep value.gnustep.plist"),
    ),
    (
        "Invalid data",
        include_bytes!("fixtures/invalid/Invalid data.openstep.plist"),
    ),
    (
        "Truncated unicode escape",
        include_bytes!("fixtures/invalid/Truncated unicode escape.openstep.plist"),
    ),
    (
        "Truncated hex escape",
        include_bytes!("fixtures/invalid/Truncated hex escape.openstep.plist"),
    ),
    (
        "Truncated octal escape",
        include_bytes!("fixtures/invalid/Truncated octal escape.openstep.plist"),
    ),
    (
        "Truncated data",
        include_bytes!("fixtures/invalid/Truncated data.openstep.plist"),
    ),
    (
        "Uneven data",
        include_bytes!("fixtures/invalid/Uneven data.openstep.plist"),
    ),
    (
        "Truncated block comment",
        include_bytes!("fixtures/invalid/Truncated block comment.openstep.plist"),
    ),
    (
        "Truncated quoted string",
        include_bytes!("fixtures/invalid/Truncated quoted string.openstep.plist"),
    ),
    (
        "Garbage after end of non-string",
        include_bytes!("fixtures/invalid/Garbage after end of non-string.openstep.plist"),
    ),
    (
        "Broken UTF-16",
        include_bytes!("fixtures/invalid/Broken UTF-16.openstep.plist"),
    ),
    (
        "Truncated GNUStep data",
        include_bytes!("fixtures/invalid/Truncated GNUStep data.gnustep.plist"),
    ),
    (
        "Truncated GNUStep base64 data (missing ])",
        include_bytes!("fixtures/invalid/Truncated GNUStep base64 data (missing ]).gnustep.plist"),
    ),
    (
        "Truncated GNUStep base64 data (missing >)",
        include_bytes!("fixtures/invalid/Truncated GNUStep base64 data (missing _).gnustep.plist"),
    ),
    (
        "Invalid GNUStep base64 data",
        include_bytes!("fixtures/invalid/Invalid GNUStep base64 data.gnustep.plist"),
    ),
    (
        "GNUStep extended value with EOF before type",
        include_bytes!(
            "fixtures/invalid/GNUStep extended value with EOF before type.gnustep.plist"
        ),
    ),
    (
        "GNUStep extended value terminated before type",
        include_bytes!(
            "fixtures/invalid/GNUStep extended value terminated before type.gnustep.plist"
        ),
    ),
    (
        "Empty GNUStep extended value",
        include_bytes!("fixtures/invalid/Empty GNUStep extended value.gnustep.plist"),
    ),
    (
        "Unterminated GNUStep quoted value",
        include_bytes!("fixtures/invalid/Unterminated GNUStep quoted value.gnustep.plist"),
    ),
    (
        "Unterminated GNUStep quoted value (EOF)",
        include_bytes!("fixtures/invalid/Unterminated GNUStep quoted value (EOF).gnustep.plist"),
    ),
    (
        "Poorly-terminated GNUStep quoted value",
        include_bytes!("fixtures/invalid/Poorly-terminated GNUStep quoted value.gnustep.plist"),
    ),
    (
        "Empty GNUStep quoted extended value",
        include_bytes!("fixtures/invalid/Empty GNUStep quoted extended value.gnustep.plist"),
    ),
];

#[test]
fn invalid_text_table_is_rejected_through_the_ladder() {
    assert_eq!(INVALID_TEXT.len(), 38);
    for (name, data) in INVALID_TEXT {
        assert_rejected(name, data);
    }
}
