//! The `TestEncode` + `TestDecode` analogue over the master corpus.
//!
//! Encode: for every entry+format not flagged `SkipEncode`, the entry's source
//! value serializes byte-exactly to the golden document. Decode: for every
//! entry+format not flagged `SkipDecode`, the golden decodes into the entry's
//! concrete type and projects to the expected `Value`; binary and XML decodes
//! are additionally cross-checked for equality.
//!
//! Per R25 the decode loop skips exactly the flagged formats deterministically
//! (the original `return`-vs-`continue` quirk is not reproduced — strictly more
//! coverage). The three float32 entries' binary-encode assertions are enabled
//! (the `Real` wide channel exists, R1/R25).

#![expect(
    clippy::panic,
    reason = "test assertions: panic surfaces a failing vector"
)]

mod common;

use apple_plist::{Format, Value, to_vec};
use common::corpus;

#[test]
fn corpus_has_sixty_eight_entries() {
    assert_eq!(corpus().len(), 68);
}

#[test]
fn entry_names_are_unique() {
    let mut names: Vec<&str> = corpus().iter().map(|entry| entry.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "duplicate entry name in corpus");
}

#[test]
fn encode_corpus() {
    for entry in corpus() {
        let Some(build) = entry.encode_value else {
            continue;
        };
        let value = build();
        for (format, doc) in entry.docs {
            if entry.skip_encode.contains(format) {
                continue;
            }
            let encoded = to_vec(&value, *format)
                .unwrap_or_else(|err| panic!("encode {} [{format}]: {err}", entry.name));
            assert_eq!(
                encoded.as_slice(),
                *doc,
                "encode mismatch for {} [{format}]",
                entry.name
            );
        }
    }
}

#[test]
fn decode_corpus() {
    for entry in corpus() {
        let expected = (entry.expected)();
        let mut binary_result: Option<Value> = None;
        let mut xml_result: Option<Value> = None;
        for (format, doc) in entry.docs {
            if entry.skip_decode.contains(format) {
                continue;
            }
            let got = (entry.decode)(doc, *format)
                .unwrap_or_else(|err| panic!("decode {} [{format}]: {err}", entry.name));
            assert_eq!(
                got, expected,
                "decode mismatch for {} [{format}]",
                entry.name
            );
            match format {
                Format::Binary => binary_result = Some(got),
                Format::Xml => xml_result = Some(got),
                Format::OpenStep | Format::GnuStep => {}
            }
        }
        // Binary and XML must agree when both ran.
        if let (Some(binary), Some(xml)) = (&binary_result, &xml_result) {
            assert_eq!(
                binary, xml,
                "{}: binary and XML decoding yielded different values",
                entry.name
            );
        }
    }
}
