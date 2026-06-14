//! Smoke tests for the public surface: type contracts, end-to-end encode
//! and decode round-trips per format, detection over golden samples, and
//! the serde derive bridge. The exhaustive corpus suites (RFC 0005) land
//! with the tests milestone.

#![expect(
    clippy::unwrap_used,
    reason = "tests assert via unwrap; a failure is the test failing"
)]

use std::time::SystemTime;

use apple_plist::{Date, Format, Integer, MAX_PARSE_DEPTH, Real, Uid, Value};

#[test]
fn format_display_matches_name() {
    assert_eq!(Format::Xml.to_string(), "XML");
    assert_eq!(Format::Binary.to_string(), "Binary");
    assert_eq!(Format::OpenStep.to_string(), "OpenStep");
    assert_eq!(Format::GnuStep.to_string(), "GNUStep");
}

#[test]
fn format_from_str_round_trips_display() {
    for format in [
        Format::Xml,
        Format::Binary,
        Format::OpenStep,
        Format::GnuStep,
    ] {
        assert_eq!(format.to_string().parse::<Format>().ok(), Some(format));
    }
    assert_eq!("xml".parse::<Format>().ok(), Some(Format::Xml));
    assert_eq!("GNUSTEP".parse::<Format>().ok(), Some(Format::GnuStep));
    assert!("bplist".parse::<Format>().is_err());
}

#[test]
fn depth_limit_matches_reference() {
    assert_eq!(MAX_PARSE_DEPTH, 128);
}

#[test]
fn uid_round_trips_its_value() {
    assert_eq!(Uid::from(42).get(), 42);
    assert_eq!(u64::from(Uid::from(42)), 42);
}

#[test]
fn integer_equality_is_numeric() {
    assert_eq!(Integer::from(5i64), Integer::from(5u64));
    assert_ne!(Integer::Signed(-1), Integer::Unsigned(u64::MAX));
}

#[test]
fn value_tree_builds_and_reads_back() {
    let value = Value::from_iter([
        ("bytes".to_owned(), Value::from(vec![0x68u8, 0x69])),
        ("narrow".to_owned(), Value::from(Real::from(32.0f32))),
        ("uid".to_owned(), Value::from(Uid::from(255))),
    ]);
    let dict = value.as_dictionary().into_iter().flatten().count();
    assert_eq!(dict, 3);
    assert_eq!(
        value
            .as_dictionary()
            .and_then(|d| d.get("narrow"))
            .and_then(Value::as_real),
        Some(32.0)
    );
}

#[test]
fn date_round_trips_through_system_time() {
    let now = SystemTime::now();
    assert_eq!(SystemTime::from(Date::from(now)), now);
}

/// End-to-end coverage over the complete default feature set.
#[cfg(all(
    feature = "serde",
    feature = "xml",
    feature = "binary",
    feature = "openstep"
))]
mod full_surface {
    use std::time::SystemTime;

    use apple_plist::{Date, Decoder, Encoder, Format, Integer, Value};

    fn sample_value() -> Value {
        Value::from_iter([
            ("blob".to_owned(), Value::from(vec![1u8, 2, 3, 4])),
            ("count".to_owned(), Value::from(42u64)),
            ("name".to_owned(), Value::from("plist")),
            ("negative".to_owned(), Value::from(Integer::Signed(-7))),
            ("pi".to_owned(), Value::from(std::f64::consts::PI)),
            ("truth".to_owned(), Value::from(true)),
            (
                "when".to_owned(),
                Value::from(Date::from(SystemTime::UNIX_EPOCH)),
            ),
        ])
    }

    #[test]
    fn every_format_round_trips_a_value_through_encoder_and_decoder() {
        for format in [
            Format::Xml,
            Format::Binary,
            Format::OpenStep,
            Format::GnuStep,
        ] {
            let value = sample_value();
            let mut document = Vec::new();
            Encoder::for_format(&mut document, format)
                .encode_value(&value)
                .unwrap();

            let mut decoder = Decoder::new(document.as_slice());
            let output = decoder.decode_value().unwrap();
            if format == Format::OpenStep {
                // OpenStep stores everything as strings; the shape survives and
                // scalars come back stringly (the serde lax path recovers them).
                let dict = output.as_dictionary().unwrap();
                assert_eq!(dict.len(), 7);
                assert_eq!(dict.get("count").and_then(Value::as_str), Some("42"));
                assert_eq!(decoder.format(), Some(Format::OpenStep));
            } else {
                assert_eq!(output, value, "{format}");
                assert_eq!(decoder.format(), Some(format));
            }
        }
    }

    /// Golden samples from the reference corpus (the `String` and `Date`
    /// corpus entries, plus `Basic Structure`).
    mod golden {
        pub(crate) const STRING_XML: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><string>Hello</string></plist>";

        pub(crate) const STRING_BINARY: &[u8] = b"bplist00UHello\x08\
            \x00\x00\x00\x00\x00\x00\x01\x01\
            \x00\x00\x00\x00\x00\x00\x00\x01\
            \x00\x00\x00\x00\x00\x00\x00\x00\
            \x00\x00\x00\x00\x00\x00\x00\x0e";

        pub(crate) const BASIC_OPENSTEP: &[u8] = b"{Name=Dustin;}";

        pub(crate) const DATE_GNUSTEP: &[u8] = b"<*D2013-11-27 00:34:00 +0000>";
    }

    #[test]
    fn detect_identifies_each_golden_sample() {
        assert_eq!(apple_plist::detect(golden::STRING_XML), Some(Format::Xml));
        assert_eq!(
            apple_plist::detect(golden::STRING_BINARY),
            Some(Format::Binary)
        );
        assert_eq!(
            apple_plist::detect(golden::BASIC_OPENSTEP),
            Some(Format::OpenStep)
        );
        assert_eq!(
            apple_plist::detect(golden::DATE_GNUSTEP),
            Some(Format::GnuStep)
        );
        assert_eq!(apple_plist::detect(b"bplist00"), None);
    }

    #[test]
    fn golden_samples_decode_and_re_encode_byte_identically() {
        let hello = Value::from("Hello");
        let decoded = Decoder::new(golden::STRING_XML).decode_value().unwrap();
        assert_eq!(decoded, hello);
        let mut encoded = Vec::new();
        Encoder::new(&mut encoded).encode_value(&hello).unwrap();
        assert_eq!(encoded, golden::STRING_XML);

        let decoded = Decoder::new(golden::STRING_BINARY).decode_value().unwrap();
        assert_eq!(decoded, hello);
        let mut encoded = Vec::new();
        Encoder::binary(&mut encoded).encode_value(&hello).unwrap();
        assert_eq!(encoded, golden::STRING_BINARY);

        let decoded = Decoder::new(golden::BASIC_OPENSTEP).decode_value().unwrap();
        let expected = Value::from_iter([("Name".to_owned(), Value::from("Dustin"))]);
        assert_eq!(decoded, expected);
        let mut encoded = Vec::new();
        Encoder::for_format(&mut encoded, Format::OpenStep)
            .encode_value(&expected)
            .unwrap();
        assert_eq!(encoded, golden::BASIC_OPENSTEP);

        // The GNUStep date golden carries the same instant as the XML form.
        let reference: Value =
            apple_plist::from_slice(b"<date>2013-11-27T00:34:00Z</date>").unwrap();
        let decoded = Decoder::new(golden::DATE_GNUSTEP).decode_value().unwrap();
        assert_eq!(decoded, reference);
        let mut encoded = Vec::new();
        Encoder::for_format(&mut encoded, Format::GnuStep)
            .encode_value(&decoded)
            .unwrap();
        assert_eq!(encoded, golden::DATE_GNUSTEP);
    }

    #[test]
    fn serde_derive_round_trips_through_every_format() {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
        struct SparseBundleHeader {
            #[serde(rename = "CFBundleInfoDictionaryVersion")]
            info_dictionary_version: String,
            #[serde(rename = "band-size")]
            band_size: u64,
            #[serde(rename = "bundle-backingstore-version")]
            backing_store_version: u64,
            #[serde(rename = "diskimage-bundle-type")]
            disk_image_bundle_type: String,
            size: u64,
        }

        let header = SparseBundleHeader {
            info_dictionary_version: "6.0".into(),
            band_size: 8_388_608,
            backing_store_version: 1,
            disk_image_bundle_type: "com.apple.diskimage.sparsebundle".into(),
            size: 4 * 1_048_576 * 1_024 * 1_024,
        };

        for format in [
            Format::Xml,
            Format::Binary,
            Format::OpenStep,
            Format::GnuStep,
        ] {
            let bytes = apple_plist::to_vec(&header, format).unwrap();
            let back: SparseBundleHeader = apple_plist::from_slice(&bytes).unwrap();
            assert_eq!(back, header, "{format}");

            let reader_back: SparseBundleHeader =
                apple_plist::from_reader(bytes.as_slice()).unwrap();
            assert_eq!(reader_back, header, "{format}");
        }
    }

    #[test]
    fn to_value_and_from_value_bridge_the_tree() {
        let tree = apple_plist::to_value(&vec![1u8, 2, 3]).unwrap();
        assert_eq!(
            tree,
            Value::Array(vec![Value::from(1u8), Value::from(2u8), Value::from(3u8),])
        );
        let back: Vec<u8> = apple_plist::from_value(tree).unwrap();
        assert_eq!(back, vec![1, 2, 3]);

        let back: Vec<u8> = apple_plist::from_value(Value::Data(vec![4, 5])).unwrap();
        assert_eq!(back, vec![4, 5]);
    }

    #[test]
    fn to_writer_streams_a_document() {
        let mut out = Vec::new();
        apple_plist::to_writer(&mut out, &"Hello", Format::Xml).unwrap();
        assert_eq!(out, golden::STRING_XML);
    }

    #[test]
    fn xml_and_binary_decodes_of_the_same_document_agree() {
        let value = sample_value();
        let xml = apple_plist::to_vec(&value, Format::Xml).unwrap();
        let binary = apple_plist::to_vec(&value, Format::Binary).unwrap();
        let from_xml: Value = apple_plist::from_slice(&xml).unwrap();
        let from_binary: Value = apple_plist::from_slice(&binary).unwrap();
        assert_eq!(from_xml, from_binary);
    }
}
