//! Derived-struct encode/decode through the serde bridge. Covers renames, skip,
//! omitempty analogues, nested structs, slices/fixed arrays, maps, `Option`
//! round-trips, the lax OpenStep-into-typed-field suite, the integer-keyed map
//! marshal rejection, flatten/embed pins (R15), the `Uid`-into-integer-field
//! downgrade, and the astral-rune marshal contract.

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test assertions: an unwrap or panic firing is the test failing"
)]

use std::collections::BTreeMap;

use apple_plist::{Date, Error, Format, Uid, Value, from_slice, to_value, to_vec};
use serde::{Deserialize, Serialize};
use time::macros::datetime;

const ALL_FORMATS: [Format; 4] = [
    Format::Xml,
    Format::Binary,
    Format::OpenStep,
    Format::GnuStep,
];

const PREAMBLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

fn xml_doc(body: &str) -> String {
    format!("{PREAMBLE}<plist version=\"1.0\">{body}</plist>")
}

fn utc_date(odt: time::OffsetDateTime) -> Date {
    Date::from(std::time::SystemTime::from(odt))
}

// --- Basic Structure ----------------------------------------------------------

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct BasicStructure {
    #[serde(rename = "Name")]
    name: String,
}

#[test]
fn basic_structure_round_trips_every_format() {
    let value = BasicStructure {
        name: "Dustin".into(),
    };
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: BasicStructure = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "round-trip in {format}");
    }
}

#[test]
fn basic_structure_golden_documents() {
    let value = BasicStructure {
        name: "Dustin".into(),
    };
    assert_eq!(to_vec(&value, Format::OpenStep).unwrap(), b"{Name=Dustin;}");
    assert_eq!(to_vec(&value, Format::GnuStep).unwrap(), b"{Name=Dustin;}");
    assert_eq!(
        to_vec(&value, Format::Xml).unwrap(),
        xml_doc("<dict><key>Name</key><string>Dustin</string></dict>").as_bytes()
    );
}

// --- Basic Structure with omitted fields (#[serde(skip)]) ---------------------

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
struct OmittedField {
    #[serde(rename = "Name")]
    name: String,
    #[serde(skip)]
    age: i64,
}

#[test]
fn skip_field_is_absent_and_decodes_to_default() {
    let value = OmittedField {
        name: "Dustin".into(),
        age: 24,
    };
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        // The skipped field never reaches the wire; decoding zeroes it.
        let decoded: OmittedField = from_slice(&bytes).unwrap();
        assert_eq!(
            decoded,
            OmittedField {
                name: "Dustin".into(),
                age: 0
            },
            "skip in {format}"
        );
    }
    assert_eq!(to_vec(&value, Format::OpenStep).unwrap(), b"{Name=Dustin;}");
}

// --- Basic Structure with empty omitempty fields ------------------------------

#[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
struct OmitEmpty {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "age", default, skip_serializing_if = "is_zero_i64")]
    age: i64,
    #[serde(rename = "Slice", default, skip_serializing_if = "Vec::is_empty")]
    slice: Vec<i64>,
    #[serde(rename = "Bool", default, skip_serializing_if = "is_false")]
    boolean: bool,
    #[serde(rename = "Uint", default, skip_serializing_if = "is_zero_u64")]
    uint: u64,
    #[serde(rename = "Float32", default, skip_serializing_if = "is_zero_f32")]
    float32: f32,
    #[serde(rename = "Float64", default, skip_serializing_if = "is_zero_f64")]
    float64: f64,
    #[serde(rename = "Stringptr", default, skip_serializing_if = "Option::is_none")]
    stringptr: Option<String>,
    #[serde(rename = "Notempty", default, skip_serializing_if = "is_zero_u64")]
    notempty: u64,
}

const fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
const fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
const fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}
const fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}
const fn is_false(v: &bool) -> bool {
    !*v
}

#[test]
fn empty_omitempty_fields_are_dropped() {
    let value = OmitEmpty {
        name: "Dustin".into(),
        notempty: 10,
        ..Default::default()
    };
    // Only `Name` and `Notempty=10` survive.
    assert_eq!(
        to_vec(&value, Format::OpenStep).unwrap(),
        b"{Name=Dustin;Notempty=10;}"
    );
    assert_eq!(
        to_vec(&value, Format::GnuStep).unwrap(),
        b"{Name=Dustin;Notempty=<*I10>;}"
    );
    assert_eq!(
        to_vec(&value, Format::Xml).unwrap(),
        xml_doc(
            "<dict><key>Name</key><string>Dustin</string><key>Notempty</key><integer>10</integer></dict>"
        )
        .as_bytes()
    );
    let bytes = to_vec(&value, Format::Xml).unwrap();
    let decoded: OmitEmpty = from_slice(&bytes).unwrap();
    assert_eq!(decoded, value);
}

// --- Pointer to structure with plist tags / SparseBundleHeader ----------------

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
struct SparseBundleHeader {
    #[serde(rename = "CFBundleInfoDictionaryVersion")]
    info_dictionary_version: String,
    #[serde(rename = "band-size")]
    band_size: u64,
    #[serde(rename = "bundle-backingstore-version")]
    backing_store_version: i64,
    #[serde(rename = "diskimage-bundle-type")]
    disk_image_bundle_type: String,
    #[serde(rename = "size")]
    size: u64,
}

fn sample_sparse_header() -> SparseBundleHeader {
    SparseBundleHeader {
        info_dictionary_version: "6.0".into(),
        band_size: 8_388_608,
        backing_store_version: 1,
        disk_image_bundle_type: "com.apple.diskimage.sparsebundle".into(),
        size: 4 * 1_048_576 * 1024 * 1024,
    }
}

#[test]
fn sparse_bundle_header_golden_documents() {
    let value = sample_sparse_header();
    assert_eq!(
        to_vec(&value, Format::OpenStep).unwrap(),
        br#"{CFBundleInfoDictionaryVersion="6.0";"band-size"=8388608;"bundle-backingstore-version"=1;"diskimage-bundle-type"="com.apple.diskimage.sparsebundle";size=4398046511104;}"#
    );
    assert_eq!(
        to_vec(&value, Format::GnuStep).unwrap(),
        b"{CFBundleInfoDictionaryVersion=6.0;band-size=<*I8388608>;bundle-backingstore-version=<*I1>;diskimage-bundle-type=com.apple.diskimage.sparsebundle;size=<*I4398046511104>;}".as_slice()
    );
    assert_eq!(
        to_vec(&value, Format::Xml).unwrap(),
        xml_doc(
            "<dict><key>CFBundleInfoDictionaryVersion</key><string>6.0</string><key>band-size</key><integer>8388608</integer><key>bundle-backingstore-version</key><integer>1</integer><key>diskimage-bundle-type</key><string>com.apple.diskimage.sparsebundle</string><key>size</key><integer>4398046511104</integer></dict>"
        )
        .as_bytes()
    );
}

#[test]
fn sparse_bundle_header_round_trips_strict_formats() {
    // OpenStep round-trip is skipped: lax can't decode strings into the numeric
    // fields, so exercise the strict formats only.
    let value = sample_sparse_header();
    for format in [Format::Xml, Format::Binary, Format::GnuStep] {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: SparseBundleHeader = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "round-trip in {format}");
    }
}

// --- Nested structs -----------------------------------------------------------

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct InnerNest {
    #[serde(rename = "FieldB")]
    field_b: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct OuterNest {
    #[serde(rename = "EmbedB")]
    embed_b: InnerNest,
    #[serde(rename = "FieldA")]
    field_a: String,
}

#[test]
fn nested_struct_round_trips_every_format() {
    let value = OuterNest {
        embed_b: InnerNest {
            field_b: "A.B.B".into(),
        },
        field_a: "A.A".into(),
    };
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: OuterNest = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "nested round-trip in {format}");
    }
}

// --- Slices, fixed arrays, [u8; N] (entries 9, 10, 11) ------------------------

#[test]
fn integer_slice_round_trips() {
    let value = vec![104_i64, 101, 108, 108, 111];
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: Vec<i64> = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "slice round-trip in {format}");
    }
}

#[test]
fn integer_fixed_array_round_trips() {
    let value = [i64::from(b'h'), i64::from(b'i'), i64::from(b'!')];
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: [i64; 3] = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "fixed-array round-trip in {format}");
    }
}

#[test]
fn byte_array_decodes_when_length_matches() {
    // Data("hello") into [u8; 5] succeeds; wrong-length targets error
    // (entry 9, <data>SGVsbG8=</data> into a 3-byte array). Note: too-large
    // [u8; 7] errors here too (deviation: zero-padding is not performed,
    // spec 05 §11.2).
    let data_doc = xml_doc("<data>aGVsbG8=</data>");
    let exact: [u8; 5] = from_slice(data_doc.as_bytes()).unwrap();
    assert_eq!(&exact, b"hello");

    let too_small: Result<[u8; 3], _> = from_slice(data_doc.as_bytes());
    assert!(too_small.is_err(), "5 bytes into [u8; 3] must error");
    let too_large: Result<[u8; 7], _> = from_slice(data_doc.as_bytes());
    assert!(
        too_large.is_err(),
        "5 bytes into [u8; 7] must error (Rust arity contract)"
    );
}

// --- Maps ---------------------------------------------------------------------

#[test]
fn string_keyed_map_round_trips() {
    let value: BTreeMap<String, String> = BTreeMap::from([
        ("Key".into(), "Value".into()),
        ("Key2".into(), "Value2".into()),
    ]);
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: BTreeMap<String, String> = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "map round-trip in {format}");
    }
}

#[test]
fn integer_keyed_map_marshal_is_rejected() {
    // "Map with integer keys": non-string map keys cannot be marshaled.
    let value: BTreeMap<i32, String> = BTreeMap::from([(1, "hi".into())]);
    assert!(to_vec(&value, Format::OpenStep).is_err());
    assert!(matches!(to_value(&value), Err(Error::UnknownType(_))));
}

// --- Option round-trips -------------------------------------------------------

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct OptionHolder {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Maybe", default, skip_serializing_if = "Option::is_none")]
    maybe: Option<i64>,
}

#[test]
fn option_some_round_trips() {
    let value = OptionHolder {
        name: "x".into(),
        maybe: Some(5),
    };
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: OptionHolder = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "Some round-trip in {format}");
    }
}

#[test]
fn option_none_is_absent_and_round_trips() {
    let value = OptionHolder {
        name: "x".into(),
        maybe: None,
    };
    let bytes = to_vec(&value, Format::Xml).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("Maybe"),
        "a None field must not appear on the wire"
    );
    let decoded: OptionHolder = from_slice(&bytes).unwrap();
    assert_eq!(decoded, value);
}

// --- Uid into integer field downgrade (entry 37) ------------------------------

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct UidStruct {
    #[serde(rename = "identifier")]
    identifier: Uid,
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
struct LegacyIntStruct {
    #[serde(rename = "identifier")]
    identifier: u64,
}

#[test]
fn uid_field_round_trips() {
    let value = UidStruct {
        identifier: Uid::from(1024_u64),
    };
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: UidStruct = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "Uid round-trip in {format}");
    }
}

#[test]
fn uid_decodes_into_integer_field() {
    // "CF Keyed Archiver UID as Legacy Int": a Uid value decodes into a u64
    // field.
    let value = UidStruct {
        identifier: Uid::from(1024_u64),
    };
    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: LegacyIntStruct = from_slice(&bytes).unwrap();
        assert_eq!(decoded.identifier, 1024, "Uid -> u64 in {format}");
    }
}

// --- Flatten / embed pins (R15: #[serde(default)] in place of embed skip) -----

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
struct EmbedInner {
    #[serde(rename = "FieldA2")]
    field_a2: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
struct EmbedOuter {
    #[serde(rename = "FieldA")]
    field_a: String,
    #[serde(flatten)]
    inner: EmbedInner,
}

#[test]
fn flatten_emits_promoted_keys() {
    // DEVIATION (R15 / RFC 0001 Risk 1): anonymous-embed promotion with
    // shallowest-wins conflict resolution is NOT replicated.
    // `#[serde(flatten)]` simply hoists the inner struct's renamed keys into the
    // outer dictionary; this test pins the actual serde behavior.
    let value = EmbedOuter {
        field_a: "A.A".into(),
        inner: EmbedInner {
            field_a2: "A.B.C.A2".into(),
        },
    };
    let projected = to_value(&value).unwrap();
    let Value::Dictionary(dict) = &projected else {
        panic!("flatten must produce a dictionary, got {projected:?}");
    };
    assert_eq!(dict.get("FieldA"), Some(&Value::from("A.A")));
    assert_eq!(dict.get("FieldA2"), Some(&Value::from("A.B.C.A2")));
    assert_eq!(dict.len(), 2);

    for format in ALL_FORMATS {
        let bytes = to_vec(&value, format).unwrap();
        let decoded: EmbedOuter = from_slice(&bytes).unwrap();
        assert_eq!(decoded, value, "flatten round-trip in {format}");
    }
}

// --- Lax suite: lax scalar coercions ------------------------------------------

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct LaxTestData {
    #[serde(rename = "I64")]
    i64: i64,
    #[serde(rename = "U64")]
    u64: u64,
    #[serde(rename = "F64")]
    f64: f64,
    #[serde(rename = "B")]
    b: bool,
    #[serde(rename = "D")]
    d: Date,
}

#[test]
fn lax_decode_struct() {
    // Through the public ladder the input detects as OpenStep, which enables lax
    // automatically. Every scalar field is a quoted/bare string coerced into its
    // typed field: int, uint, float, bool, and a text-layout date.
    let input = br#"{B=1;D="2013-11-27 00:34:00 +0000";I64=1;F64="3.0";U64=2;}"#;
    let decoded: LaxTestData = from_slice(input).unwrap();
    let expected = LaxTestData {
        i64: 1,
        u64: 2,
        f64: 3.0,
        b: true,
        d: utc_date(datetime!(2013-11-27 00:34:00 UTC)),
    };
    assert_eq!(decoded, expected);
}

// --- Lax suite: illegal lax coercions -----------------------------------------

#[test]
fn illegal_lax_coercions_error() {
    // Bare OpenStep strings that cannot coerce into the requested scalar must
    // error, never panic. The fifth row (a byte-slice destination) becomes a
    // Vec<u8> target.
    let signed: Result<i64, _> = from_slice(b"abc");
    assert!(signed.is_err(), "\"abc\" into i64");
    let unsigned: Result<u64, _> = from_slice(b"abc");
    assert!(unsigned.is_err(), "\"abc\" into u64");
    let float: Result<f64, _> = from_slice(b"def");
    assert!(float.is_err(), "\"def\" into f64");
    let boolean: Result<bool, _> = from_slice(b"ghi");
    assert!(boolean.is_err(), "\"ghi\" into bool");
    let bytes: Result<Vec<u8>, _> = from_slice(b"jkl");
    assert!(bytes.is_err(), "\"jkl\" into Vec<u8>");
}

#[test]
fn lax_bool_token_table() {
    // The twelve C-style boolean tokens are accepted; everything else errors
    // (spec 05 §6).
    for token in ["1", "t", "T", "TRUE", "true", "True"] {
        let decoded: bool = from_slice(token.as_bytes()).unwrap_or_else(|e| {
            panic!("lax bool token {token:?} should parse true: {e}");
        });
        assert!(decoded, "{token:?} -> true");
    }
    for token in ["0", "f", "F", "FALSE", "false", "False"] {
        let decoded: bool = from_slice(token.as_bytes()).unwrap_or_else(|e| {
            panic!("lax bool token {token:?} should parse false: {e}");
        });
        assert!(!decoded, "{token:?} -> false");
    }
    for token in ["yes", "2", "TrUe"] {
        let decoded: Result<bool, _> = from_slice(token.as_bytes());
        assert!(decoded.is_err(), "{token:?} is not a ParseBool token");
    }
}

#[test]
fn lax_unsigned_rejects_sign_prefix() {
    // Unsigned parsing forbids a sign prefix (spec 05 §6); signed accepts both.
    let plus_unsigned: Result<u64, _> = from_slice(b"+5");
    assert!(
        plus_unsigned.is_err(),
        "\"+5\" into u64 (ParseUint sign rule)"
    );
    let plus_signed: i64 = from_slice(b"+5").unwrap();
    assert_eq!(plus_signed, 5);
    let minus_signed: i64 = from_slice(b"-5").unwrap();
    assert_eq!(minus_signed, -5);
    // Base 10 only: no 0x sniffing in lax integers (R17).
    let hex_signed: Result<i64, _> = from_slice(b"0x10");
    assert!(hex_signed.is_err(), "\"0x10\" into i64 (base 10 only)");
    let hex_unsigned: Result<u64, _> = from_slice(b"0x10");
    assert!(hex_unsigned.is_err(), "\"0x10\" into u64 (base 10 only)");
}

#[test]
fn lax_float_grammar_edges() {
    let three: f64 = from_slice(b"3.0").unwrap();
    assert!((three - 3.0).abs() < f64::EPSILON);
    let exp: f64 = from_slice(b"1e3").unwrap();
    assert!((exp - 1000.0).abs() < f64::EPSILON);
    let neg_inf: f64 = from_slice(br#""-Inf""#).unwrap();
    assert!(neg_inf.is_infinite() && neg_inf.is_sign_negative());
    let nan: f64 = from_slice(b"nan").unwrap();
    assert!(nan.is_nan());
    // Range rule: an overflow to infinity that wasn't spelled "inf" errors.
    let overflow: Result<f64, _> = from_slice(b"1e999");
    assert!(overflow.is_err(), "\"1e999\" overflows to +Inf (ErrRange)");
}

#[test]
fn lax_never_coerces_into_value() {
    // Decoding into `Value`/`BTreeMap<String, Value>` keeps OpenStep scalars as
    // strings — lax fires only for concrete scalar targets.
    let value: Value = from_slice(br#"{a="1";}"#).unwrap();
    let Value::Dictionary(dict) = &value else {
        panic!("expected a dictionary, got {value:?}");
    };
    assert_eq!(dict.get("a"), Some(&Value::from("1")));
}

// --- Strict type mismatches ---------------------------------------------------

#[test]
fn strict_type_mismatches_error() {
    let cases: &[(&str, &str)] = &[
        ("<string>abc</string>", "string -> i64"),
        ("<data>ABC=</data>", "data -> i64"),
        ("<real>34.1</real>", "real -> i64"),
        ("<true>def</true>", "bool -> i64"),
        ("<date>2010-01-01T00:00:00Z</date>", "date -> i64"),
    ];
    for (body, label) in cases {
        let doc = xml_doc(body);
        let decoded: Result<i64, _> = from_slice(doc.as_bytes());
        assert!(decoded.is_err(), "{label} must error");
    }

    let int_into_bool: Result<bool, _> = from_slice(xml_doc("<integer>0</integer>").as_bytes());
    assert!(int_into_bool.is_err(), "integer -> bool must error");
    let array_into_bool: Result<bool, _> =
        from_slice(xml_doc("<array><integer>0</integer></array>").as_bytes());
    assert!(array_into_bool.is_err(), "array -> bool must error");
    let dict_into_bool: Result<bool, _> =
        from_slice(xml_doc("<dict><key>a</key><integer>0</integer></dict>").as_bytes());
    assert!(dict_into_bool.is_err(), "dict -> bool must error");

    let three_bools_into_one: Result<[i64; 1], _> =
        from_slice(xml_doc("<array><true/><true/><true/></array>").as_bytes());
    assert!(
        three_bools_into_one.is_err(),
        "3-element array -> [i64; 1] must error"
    );
    let five_bytes_into_three: Result<[u8; 3], _> =
        from_slice(xml_doc("<data>SGVsbG8=</data>").as_bytes());
    assert!(
        five_bytes_into_three.is_err(),
        "5-byte data -> [u8; 3] must error"
    );
}

#[test]
fn integer_into_float_errors_real_into_float_downcasts() {
    // integer -> float errors, but real -> f32 downcasts silently.
    let int_into_f64: Result<f64, _> = from_slice(xml_doc("<integer>5</integer>").as_bytes());
    assert!(int_into_f64.is_err(), "integer -> f64 must error");
    let real_into_f32: f32 = from_slice(xml_doc("<real>3.5</real>").as_bytes()).unwrap();
    assert!((real_into_f32 - 3.5).abs() < f32::EPSILON);
}

// --- Numeric width / sign range checking (deviation 11.1) ---------------------

#[test]
fn narrow_integer_decode_is_range_checked() {
    // DEVIATION (R14 / spec 05 §11.1): silent wrapping (300 -> i8 = 44) is not
    // performed; serde's visitors range-check and error instead.
    let three_hundred_into_i8: Result<i8, _> =
        from_slice(xml_doc("<integer>300</integer>").as_bytes());
    assert!(
        three_hundred_into_i8.is_err(),
        "300 -> i8 must error (range-checked)"
    );

    let max_u64_into_i64: Result<i64, _> =
        from_slice(xml_doc("<integer>18446744073709551615</integer>").as_bytes());
    assert!(
        max_u64_into_i64.is_err(),
        "u64::MAX -> i64 must error (range-checked)"
    );

    // In-range narrowings succeed.
    let small_into_i8: i8 = from_slice(xml_doc("<integer>5</integer>").as_bytes()).unwrap();
    assert_eq!(small_into_i8, 5);
}

// --- Astral-rune marshal contract ---------------------------------------------

#[test]
fn astral_rune_marshal_contract() {
    const VALUE: &str = "grin \u{1F600} end";
    let map: BTreeMap<String, String> = BTreeMap::from([("e".into(), VALUE.into())]);

    // XML and Binary are faithful.
    for format in [Format::Xml, Format::Binary] {
        let bytes = to_vec(&map, format).unwrap();
        let decoded: BTreeMap<String, String> = from_slice(&bytes).unwrap();
        assert_eq!(
            decoded.get("e").map(String::as_str),
            Some(VALUE),
            "{format} faithful"
        );
    }

    // OpenStep / GNUStep encode without panic but are knowingly lossy: the
    // astral rune is a surrogate pair the 4-hex-digit text parser cannot
    // reassemble. If these ever round-trip faithfully, surrogate pairs were
    // implemented — update the test.
    for format in [Format::OpenStep, Format::GnuStep] {
        let bytes = to_vec(&map, format).unwrap();
        let decoded: BTreeMap<String, String> = from_slice(&bytes).unwrap();
        assert_ne!(
            decoded.get("e").map(String::as_str),
            Some(VALUE),
            "{format} astral round-trip must be lossy"
        );
    }
}
