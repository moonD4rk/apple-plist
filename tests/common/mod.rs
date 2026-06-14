//! Shared master test vector: the 68-entry corpus, with per-format fixtures,
//! skip flags, encode source values, and typed-decode projections. Consumed by
//! `roundtrip.rs` (the `TestEncode`/`TestDecode` analogue).
//!
//! Each entry mirrors one `TestData`. Where the source value was a struct or
//! pointer (reflection-specific), the entry carries the `Value`-tree
//! equivalent per spec 07's porting matrix; those rows are recorded as
//! deviations in the porting report.

use std::time::{Duration, SystemTime};

use apple_plist::{Date, Dictionary, Format, Integer, Real, Uid, Value, from_slice, to_value};

/// One corpus entry: a named value plus its per-format golden documents and
/// skip flags.
pub(crate) struct Entry {
    /// The `TestData.Name`, kept verbatim (also the subtest label).
    pub(crate) name: &'static str,
    /// The format-tagged golden documents present for this entry. A format
    /// absent here is simply not exercised (no encode, no decode).
    pub(crate) docs: &'static [(Format, &'static [u8])],
    /// Formats whose `SkipEncode` flag is set: the fixture exists but must not
    /// be encode-compared (decode-only).
    pub(crate) skip_encode: &'static [Format],
    /// Formats whose `SkipDecode` flag is set: the fixture exists but must not
    /// be decode-compared (encode-only).
    pub(crate) skip_decode: &'static [Format],
    /// The source value to encode, when the entry participates in encoding.
    /// `None` marks a purely decode-only entry whose source value is not
    /// faithfully expressible as an encode source (e.g. the UTF-16 input
    /// documents).
    pub(crate) encode_value: Option<fn() -> Value>,
    /// Decodes `doc` (detected as `format`) into the entry's concrete Rust
    /// type, then projects back to a `Value` for comparison. The `format`
    /// argument is unused by most entries but lets the rare per-format target
    /// vary if needed.
    pub(crate) decode: fn(&[u8], Format) -> apple_plist::Result<Value>,
    /// The `Value` projection every successful decode must equal.
    pub(crate) expected: fn() -> Value,
}

// --- decode projections -----------------------------------------------------

/// Decodes into `T` through the public ladder, then projects to a `Value`,
/// exactly as `TestDecode` normalizes both sides before a deep comparison.
fn decode_typed<T>(doc: &[u8], _format: Format) -> apple_plist::Result<Value>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value: T = from_slice(doc)?;
    to_value(&value)
}

/// Decodes straight into a `Value` tree (no scalar coercion); used where every
/// format already produces the right value kind.
fn decode_as_value(doc: &[u8], _format: Format) -> apple_plist::Result<Value> {
    let value: Value = from_slice(doc)?;
    to_value(&value)
}

// --- typed mirror structs ---------------------------------------------------

/// Entry 6's decode target: the two non-empty fields survive the wire, and the
/// OpenStep document delivers `Notempty` as the lax string `"10"`. Mirrors the
/// typed-decode target struct (the omitempty fields are absent on the wire, so
/// they need no Rust presence — spec §2 row 6 / R15).
#[derive(serde::Serialize, serde::Deserialize)]
struct EmptyOmitempty {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Notempty")]
    notempty: u64,
}

// --- value-tree builders ----------------------------------------------------

fn date_2013() -> Date {
    // 2013-11-27T00:34:00Z == Unix 1_385_512_440.
    Date::from(SystemTime::UNIX_EPOCH + Duration::from_secs(1_385_512_440))
}

fn str_array(items: &[&str]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|s| Value::String((*s).to_owned()))
            .collect(),
    )
}

fn int_array_signed(items: &[i64]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|n| Value::Integer(Integer::Signed(*n)))
            .collect(),
    )
}

fn uint_array(items: &[u64]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|n| Value::Integer(Integer::Unsigned(*n)))
            .collect(),
    )
}

// Corpus dictionaries are materialized in byte-wise key order so the
// insertion-order-preserving encoder reproduces the golden fixtures, which carry
// sorted keys.
fn dict(entries: &[(&str, Value)]) -> Value {
    let mut pairs: Vec<(String, Value)> = entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Dictionary(pairs.into_iter().collect())
}

fn string_dict(entries: &[(&str, &str)]) -> Value {
    let mut pairs: Vec<(String, Value)> = entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned())))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Dictionary(pairs.into_iter().collect())
}

// --- include_bytes shorthands ----------------------------------------------

macro_rules! doc {
    ($fmt:expr, $path:literal) => {{
        const BYTES: &[u8] = include_bytes!(concat!("../fixtures/corpus/", $path));
        ($fmt, BYTES)
    }};
}

// ===========================================================================
// The 68-entry table.
// ===========================================================================

/// Returns the full corpus, in master-table declaration order.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one literal table row per TestData entry; splitting it hides the 1:1 mapping"
)]
pub(crate) fn corpus() -> Vec<Entry> {
    vec![
        // 1. String
        Entry {
            name: "String",
            docs: &[
                doc!(Format::Xml, "String.xml.plist"),
                doc!(Format::Binary, "String.binary.plist"),
                doc!(Format::OpenStep, "String.openstep.plist"),
                doc!(Format::GnuStep, "String.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::String("Hello".to_owned())),
            decode: decode_typed::<String>,
            expected: || Value::String("Hello".to_owned()),
        },
        // 2. String containing apostrophe
        Entry {
            name: "String containing apostrophe",
            docs: &[
                doc!(Format::Xml, "String containing apostrophe.xml.plist"),
                doc!(Format::Binary, "String containing apostrophe.binary.plist"),
                doc!(
                    Format::OpenStep,
                    "String containing apostrophe.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "String containing apostrophe.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::String("'".to_owned())),
            decode: decode_typed::<String>,
            expected: || Value::String("'".to_owned()),
        },
        // 3. Basic Structure
        Entry {
            name: "Basic Structure",
            docs: &[
                doc!(Format::Xml, "Basic Structure.xml.plist"),
                doc!(Format::Binary, "Basic Structure.binary.plist"),
                doc!(Format::OpenStep, "Basic Structure.openstep.plist"),
                doc!(Format::GnuStep, "Basic Structure.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| dict(&[("Name", Value::String("Dustin".to_owned()))])),
            decode: decode_as_value,
            expected: || dict(&[("Name", Value::String("Dustin".to_owned()))]),
        },
        // 4. Basic Structure with non-exported fields (typed decode drops `age`)
        Entry {
            name: "Basic Structure with non-exported fields",
            docs: &[
                doc!(
                    Format::Xml,
                    "Basic Structure with non-exported fields.xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "Basic Structure with non-exported fields.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Basic Structure with non-exported fields.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Basic Structure with non-exported fields.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| dict(&[("Name", Value::String("Dustin".to_owned()))])),
            decode: decode_as_value,
            expected: || dict(&[("Name", Value::String("Dustin".to_owned()))]),
        },
        // 5. Basic Structure with omitted fields (fields tagged to skip)
        Entry {
            name: "Basic Structure with omitted fields",
            docs: &[
                doc!(Format::Xml, "Basic Structure with omitted fields.xml.plist"),
                doc!(
                    Format::Binary,
                    "Basic Structure with omitted fields.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Basic Structure with omitted fields.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Basic Structure with omitted fields.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| dict(&[("Name", Value::String("Dustin".to_owned()))])),
            decode: decode_as_value,
            expected: || dict(&[("Name", Value::String("Dustin".to_owned()))]),
        },
        // 6. Basic Structure with empty omitempty fields
        Entry {
            name: "Basic Structure with empty omitempty fields",
            docs: &[
                doc!(
                    Format::Xml,
                    "Basic Structure with empty omitempty fields.xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "Basic Structure with empty omitempty fields.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Basic Structure with empty omitempty fields.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Basic Structure with empty omitempty fields.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| {
                dict(&[
                    ("Name", Value::String("Dustin".to_owned())),
                    ("Notempty", Value::Integer(Integer::Unsigned(10))),
                ])
            }),
            decode: decode_typed::<EmptyOmitempty>,
            expected: || {
                dict(&[
                    ("Name", Value::String("Dustin".to_owned())),
                    ("Notempty", Value::Integer(Integer::Unsigned(10))),
                ])
            },
        },
        // 7. Structure with Anonymous Embeds (Value-level oracle, spec §2.2)
        Entry {
            name: "Structure with Anonymous Embeds",
            docs: &[
                doc!(Format::Xml, "Structure with Anonymous Embeds.xml.plist"),
                doc!(
                    Format::Binary,
                    "Structure with Anonymous Embeds.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Structure with Anonymous Embeds.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Structure with Anonymous Embeds.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(embed_value),
            decode: decode_as_value,
            expected: embed_value,
        },
        // 8. Arbitrary Byte Data (byte slice)
        Entry {
            name: "Arbitrary Byte Data",
            docs: &[
                doc!(Format::Xml, "Arbitrary Byte Data.xml.plist"),
                doc!(Format::Binary, "Arbitrary Byte Data.binary.plist"),
                doc!(Format::OpenStep, "Arbitrary Byte Data.openstep.plist"),
                doc!(Format::GnuStep, "Arbitrary Byte Data.gnustep.plist"),
            ],
            skip_encode: &[Format::GnuStep],
            skip_decode: &[],
            encode_value: Some(|| Value::Data(b"hello".to_vec())),
            decode: decode_as_value,
            expected: || Value::Data(b"hello".to_vec()),
        },
        // 9. Arbitrary Byte Data (array) — identical docs to #8
        Entry {
            name: "Arbitrary Byte Data (array)",
            docs: &[
                doc!(Format::Xml, "Arbitrary Byte Data (array).xml.plist"),
                doc!(Format::Binary, "Arbitrary Byte Data (array).binary.plist"),
                doc!(
                    Format::OpenStep,
                    "Arbitrary Byte Data (array).openstep.plist"
                ),
                doc!(Format::GnuStep, "Arbitrary Byte Data (array).gnustep.plist"),
            ],
            skip_encode: &[Format::GnuStep],
            skip_decode: &[],
            encode_value: Some(|| Value::Data(b"hello".to_vec())),
            decode: decode_as_value,
            expected: || Value::Data(b"hello".to_vec()),
        },
        // 10. Arbitrary Integer Slice
        Entry {
            name: "Arbitrary Integer Slice",
            docs: &[
                doc!(Format::Xml, "Arbitrary Integer Slice.xml.plist"),
                doc!(Format::Binary, "Arbitrary Integer Slice.binary.plist"),
                doc!(Format::OpenStep, "Arbitrary Integer Slice.openstep.plist"),
                doc!(Format::GnuStep, "Arbitrary Integer Slice.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| int_array_signed(&[104, 101, 108, 108, 111])),
            decode: decode_typed::<Vec<i64>>,
            expected: || int_array_signed(&[104, 101, 108, 108, 111]),
        },
        // 11. Arbitrary Integer Array
        Entry {
            name: "Arbitrary Integer Array",
            docs: &[
                doc!(Format::Xml, "Arbitrary Integer Array.xml.plist"),
                doc!(Format::Binary, "Arbitrary Integer Array.binary.plist"),
                doc!(Format::OpenStep, "Arbitrary Integer Array.openstep.plist"),
                doc!(Format::GnuStep, "Arbitrary Integer Array.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| int_array_signed(&[104, 105, 33])),
            decode: decode_typed::<Vec<i64>>,
            expected: || int_array_signed(&[104, 105, 33]),
        },
        // 12. Unsigned Integers of Increasing Size
        Entry {
            name: "Unsigned Integers of Increasing Size",
            docs: &[
                doc!(
                    Format::Xml,
                    "Unsigned Integers of Increasing Size.xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "Unsigned Integers of Increasing Size.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Unsigned Integers of Increasing Size.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Unsigned Integers of Increasing Size.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(unsigned_increasing),
            decode: decode_typed::<Vec<u64>>,
            expected: unsigned_increasing,
        },
        // 13. Hexadecimal Integers (decode-only XML)
        Entry {
            name: "Hexadecimal Integers",
            docs: &[doc!(Format::Xml, "Hexadecimal Integers.xml.plist")],
            skip_encode: &[Format::Xml],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<Vec<i64>>,
            expected: || int_array_signed(&[0x68, 0x65, 0x78, 0x69, 0x6e, 0x74, -0x2a]),
        },
        // 14. Octal Integers (treated as Decimal) (decode-only XML)
        Entry {
            name: "Octal Integers (treated as Decimal)",
            docs: &[doc!(
                Format::Xml,
                "Octal Integers (treated as Decimal).xml.plist"
            )],
            skip_encode: &[Format::Xml],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<Vec<i64>>,
            expected: || int_array_signed(&[111, 99, 116, 105, 110, 116, -42]),
        },
        // 15. Floats of Increasing Bitness (WIDE-REAL; text decode skipped)
        Entry {
            name: "Floats of Increasing Bitness",
            docs: &[
                doc!(Format::Xml, "Floats of Increasing Bitness.xml.plist"),
                doc!(Format::Binary, "Floats of Increasing Bitness.binary.plist"),
                doc!(
                    Format::OpenStep,
                    "Floats of Increasing Bitness.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Floats of Increasing Bitness.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[Format::Xml, Format::OpenStep, Format::GnuStep],
            encode_value: Some(|| {
                Value::Array(vec![
                    Value::Real(Real::from(f32::MAX)),
                    Value::Real(Real::from(f64::MAX)),
                ])
            }),
            decode: decode_as_value,
            expected: || {
                Value::Array(vec![
                    Value::Real(Real::from(f32::MAX)),
                    Value::Real(Real::from(f64::MAX)),
                ])
            },
        },
        // 16. Boolean True
        Entry {
            name: "Boolean True",
            docs: &[
                doc!(Format::Xml, "Boolean True.xml.plist"),
                doc!(Format::Binary, "Boolean True.binary.plist"),
                doc!(Format::OpenStep, "Boolean True.openstep.plist"),
                doc!(Format::GnuStep, "Boolean True.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Boolean(true)),
            decode: decode_typed::<bool>,
            expected: || Value::Boolean(true),
        },
        // 17. Floating-Point Value
        Entry {
            name: "Floating-Point Value",
            docs: &[
                doc!(Format::Xml, "Floating-Point Value.xml.plist"),
                doc!(Format::Binary, "Floating-Point Value.binary.plist"),
                doc!(Format::OpenStep, "Floating-Point Value.openstep.plist"),
                doc!(Format::GnuStep, "Floating-Point Value.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Real(Real::from(std::f64::consts::PI))),
            decode: decode_typed::<f64>,
            expected: || Value::Real(Real::from(std::f64::consts::PI)),
        },
        // 18. Map (containing arbitrary types); OpenStep decode skipped
        Entry {
            name: "Map (containing arbitrary types)",
            docs: &[
                doc!(Format::Xml, "Map (containing arbitrary types).xml.plist"),
                doc!(
                    Format::Binary,
                    "Map (containing arbitrary types).binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Map (containing arbitrary types).openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Map (containing arbitrary types).gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[Format::OpenStep],
            encode_value: Some(|| {
                dict(&[
                    ("float", Value::Real(Real::from(1.0_f64))),
                    ("uint64", Value::Integer(Integer::Unsigned(1))),
                ])
            }),
            decode: decode_as_value,
            expected: || {
                dict(&[
                    ("float", Value::Real(Real::from(1.0_f64))),
                    ("uint64", Value::Integer(Integer::Unsigned(1))),
                ])
            },
        },
        // 19. Map (containing all variations of all types) — encode-only ×4
        Entry {
            name: "Map (containing all variations of all types)",
            docs: &[
                doc!(
                    Format::Xml,
                    "Map (containing all variations of all types).xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "Map (containing all variations of all types).binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Map (containing all variations of all types).openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Map (containing all variations of all types).gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[
                Format::Xml,
                Format::Binary,
                Format::OpenStep,
                Format::GnuStep,
            ],
            encode_value: Some(map_all_variations),
            decode: decode_as_value,
            expected: map_all_variations,
        },
        // 20. Map (containing nil) — typed decode drops the nil entry
        Entry {
            name: "Map (containing nil)",
            docs: &[
                doc!(Format::Xml, "Map (containing nil).xml.plist"),
                doc!(Format::Binary, "Map (containing nil).binary.plist"),
                doc!(Format::OpenStep, "Map (containing nil).openstep.plist"),
                doc!(Format::GnuStep, "Map (containing nil).gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[Format::OpenStep],
            encode_value: Some(|| {
                dict(&[
                    ("float", Value::Real(Real::from(1.5_f64))),
                    ("uint64", Value::Integer(Integer::Unsigned(1))),
                ])
            }),
            decode: decode_as_value,
            expected: || {
                dict(&[
                    ("float", Value::Real(Real::from(1.5_f64))),
                    ("uint64", Value::Integer(Integer::Unsigned(1))),
                ])
            },
        },
        // 21. Pointer to structure with plist tags; OpenStep decode skipped
        Entry {
            name: "Pointer to structure with plist tags",
            docs: &[
                doc!(
                    Format::Xml,
                    "Pointer to structure with plist tags.xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "Pointer to structure with plist tags.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Pointer to structure with plist tags.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Pointer to structure with plist tags.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[Format::OpenStep],
            encode_value: Some(sparse_bundle),
            decode: decode_as_value,
            expected: sparse_bundle,
        },
        // 22. Array of byte arrays
        Entry {
            name: "Array of byte arrays",
            docs: &[
                doc!(Format::Xml, "Array of byte arrays.xml.plist"),
                doc!(Format::Binary, "Array of byte arrays.binary.plist"),
                doc!(Format::OpenStep, "Array of byte arrays.openstep.plist"),
                doc!(Format::GnuStep, "Array of byte arrays.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| {
                Value::Array(vec![
                    Value::Data(b"Hello".to_vec()),
                    Value::Data(b"World".to_vec()),
                ])
            }),
            decode: decode_as_value,
            expected: || {
                Value::Array(vec![
                    Value::Data(b"Hello".to_vec()),
                    Value::Data(b"World".to_vec()),
                ])
            },
        },
        // 23. Date — OpenStep decode is typed (lax string -> date), spec §2.5
        Entry {
            name: "Date",
            docs: &[
                doc!(Format::Xml, "Date.xml.plist"),
                doc!(Format::Binary, "Date.binary.plist"),
                doc!(Format::OpenStep, "Date.openstep.plist"),
                doc!(Format::GnuStep, "Date.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Date(date_2013())),
            decode: decode_typed::<Date>,
            expected: || Value::Date(date_2013()),
        },
        // 24. Floating-Point NaN — encode-only ×4 (NaN != NaN)
        Entry {
            name: "Floating-Point NaN",
            docs: &[
                doc!(Format::Xml, "Floating-Point NaN.xml.plist"),
                doc!(Format::Binary, "Floating-Point NaN.binary.plist"),
                doc!(Format::OpenStep, "Floating-Point NaN.openstep.plist"),
                doc!(Format::GnuStep, "Floating-Point NaN.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[
                Format::Xml,
                Format::Binary,
                Format::OpenStep,
                Format::GnuStep,
            ],
            encode_value: Some(|| Value::Real(Real::from(f64::from_bits(0x7FF8_0000_0000_0001)))),
            decode: decode_as_value,
            expected: || Value::Real(Real::from(f64::from_bits(0x7FF8_0000_0000_0001))),
        },
        // 25. Floating-Point Infinity
        Entry {
            name: "Floating-Point Infinity",
            docs: &[
                doc!(Format::Xml, "Floating-Point Infinity.xml.plist"),
                doc!(Format::Binary, "Floating-Point Infinity.binary.plist"),
                doc!(Format::OpenStep, "Floating-Point Infinity.openstep.plist"),
                doc!(Format::GnuStep, "Floating-Point Infinity.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Real(Real::from(f64::INFINITY))),
            decode: decode_typed::<f64>,
            expected: || Value::Real(Real::from(f64::INFINITY)),
        },
        // 26. Floating-Point Negative Infinity
        Entry {
            name: "Floating-Point Negative Infinity",
            docs: &[
                doc!(Format::Xml, "Floating-Point Negative Infinity.xml.plist"),
                doc!(
                    Format::Binary,
                    "Floating-Point Negative Infinity.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "Floating-Point Negative Infinity.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Floating-Point Negative Infinity.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Real(Real::from(f64::NEG_INFINITY))),
            decode: decode_typed::<f64>,
            expected: || Value::Real(Real::from(f64::NEG_INFINITY)),
        },
        // 27. UTF-8 string
        Entry {
            name: "UTF-8 string",
            docs: &[
                doc!(Format::Xml, "UTF-8 string.xml.plist"),
                doc!(Format::Binary, "UTF-8 string.binary.plist"),
                doc!(Format::OpenStep, "UTF-8 string.openstep.plist"),
                doc!(Format::GnuStep, "UTF-8 string.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| str_array(&["Hello, ASCII", "Hello, 世界"])),
            decode: decode_typed::<Vec<String>>,
            expected: || str_array(&["Hello, ASCII", "Hello, 世界"]),
        },
        // 28. An array containing more than fifteen items
        Entry {
            name: "An array containing more than fifteen items",
            docs: &[
                doc!(
                    Format::Xml,
                    "An array containing more than fifteen items.xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "An array containing more than fifteen items.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "An array containing more than fifteen items.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "An array containing more than fifteen items.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| {
                int_array_signed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
            }),
            decode: decode_typed::<Vec<i64>>,
            expected: || int_array_signed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        },
        // 29. TextMarshaler/TextUnmarshaler -> "truthful"
        Entry {
            name: "TextMarshaler/TextUnmarshaler",
            docs: &[
                doc!(Format::Xml, "TextMarshaler_TextUnmarshaler.xml.plist"),
                doc!(Format::Binary, "TextMarshaler_TextUnmarshaler.binary.plist"),
                doc!(
                    Format::OpenStep,
                    "TextMarshaler_TextUnmarshaler.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "TextMarshaler_TextUnmarshaler.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::String("truthful".to_owned())),
            decode: decode_typed::<String>,
            expected: || Value::String("truthful".to_owned()),
        },
        // 30. TextMarshaler/TextUnmarshaler via Pointer -> "unimaginable"
        Entry {
            name: "TextMarshaler/TextUnmarshaler via Pointer",
            docs: &[
                doc!(
                    Format::Xml,
                    "TextMarshaler_TextUnmarshaler via Pointer.xml.plist"
                ),
                doc!(
                    Format::Binary,
                    "TextMarshaler_TextUnmarshaler via Pointer.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "TextMarshaler_TextUnmarshaler via Pointer.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "TextMarshaler_TextUnmarshaler via Pointer.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::String("unimaginable".to_owned())),
            decode: decode_typed::<String>,
            expected: || Value::String("unimaginable".to_owned()),
        },
        // 31. Duplicated Values (binary-only; dedup ref table, WIDE-REAL)
        Entry {
            name: "Duplicated Values",
            docs: &[doc!(Format::Binary, "Duplicated Values.binary.plist")],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(duplicated_values),
            decode: decode_as_value,
            expected: duplicated_values,
        },
        // 32. Funny Characters (O + G; string-to-string map)
        Entry {
            name: "Funny Characters",
            docs: &[
                doc!(Format::OpenStep, "Funny Characters.openstep.plist"),
                doc!(Format::GnuStep, "Funny Characters.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(funny_characters),
            decode: decode_as_value,
            expected: funny_characters,
        },
        // 33. Signed Integers
        Entry {
            name: "Signed Integers",
            docs: &[
                doc!(Format::Xml, "Signed Integers.xml.plist"),
                doc!(Format::Binary, "Signed Integers.binary.plist"),
                doc!(Format::OpenStep, "Signed Integers.openstep.plist"),
                doc!(Format::GnuStep, "Signed Integers.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| int_array_signed(&[-1, -127, -255, -32767, -65535, i64::MIN])),
            decode: decode_typed::<Vec<i64>>,
            expected: || int_array_signed(&[-1, -127, -255, -32767, -65535, i64::MIN]),
        },
        // 34. A map with a blank key
        Entry {
            name: "A map with a blank key",
            docs: &[
                doc!(Format::Xml, "A map with a blank key.xml.plist"),
                doc!(Format::Binary, "A map with a blank key.binary.plist"),
                doc!(Format::OpenStep, "A map with a blank key.openstep.plist"),
                doc!(Format::GnuStep, "A map with a blank key.gnustep.plist"),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| string_dict(&[("", "Hello")])),
            decode: decode_as_value,
            expected: || string_dict(&[("", "Hello")]),
        },
        // 35. CF Keyed Archiver UIDs (any) -> Vec<Uid>
        Entry {
            name: "CF Keyed Archiver UIDs (any)",
            docs: &[
                doc!(Format::Xml, "CF Keyed Archiver UIDs (any).xml.plist"),
                doc!(Format::Binary, "CF Keyed Archiver UIDs (any).binary.plist"),
                doc!(
                    Format::OpenStep,
                    "CF Keyed Archiver UIDs (any).openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "CF Keyed Archiver UIDs (any).gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(cf_uids),
            decode: decode_as_value,
            expected: cf_uids,
        },
        // 36. CF Keyed Archiver UID (struct)
        Entry {
            name: "CF Keyed Archiver UID (struct)",
            docs: &[
                doc!(Format::Xml, "CF Keyed Archiver UID (struct).xml.plist"),
                doc!(
                    Format::Binary,
                    "CF Keyed Archiver UID (struct).binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "CF Keyed Archiver UID (struct).openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "CF Keyed Archiver UID (struct).gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| dict(&[("identifier", Value::Uid(Uid::from(1024)))])),
            decode: decode_as_value,
            expected: || dict(&[("identifier", Value::Uid(Uid::from(1024)))]),
        },
        // 37. CF Keyed Archiver UID as Legacy Int (typed decode: identifier -> u64)
        Entry {
            name: "CF Keyed Archiver UID as Legacy Int",
            docs: &[
                doc!(Format::Xml, "CF Keyed Archiver UID as Legacy Int.xml.plist"),
                doc!(
                    Format::Binary,
                    "CF Keyed Archiver UID as Legacy Int.binary.plist"
                ),
                doc!(
                    Format::OpenStep,
                    "CF Keyed Archiver UID as Legacy Int.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "CF Keyed Archiver UID as Legacy Int.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| dict(&[("identifier", Value::Uid(Uid::from(1024)))])),
            decode: decode_as_value,
            // UID decodes to UID at Value level; the u64 downgrade is a typed-decode
            // concern exercised in custom_serde.rs, not at Value projection.
            expected: || dict(&[("identifier", Value::Uid(Uid::from(1024)))]),
        },
        // 38. Custom Marshaller/Unmarshaller by Value (G only)
        Entry {
            name: "Custom Marshaller/Unmarshaller by Value",
            docs: &[doc!(
                Format::GnuStep,
                "Custom Marshaller_Unmarshaller by Value.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| {
                Value::Array(vec![
                    Value::Integer(Integer::Unsigned(100)),
                    uint_array(&[2, 4, 6, 8]),
                ])
            }),
            decode: decode_as_value,
            expected: || {
                Value::Array(vec![
                    Value::Integer(Integer::Unsigned(100)),
                    uint_array(&[2, 4, 6, 8]),
                ])
            },
        },
        // 39. Custom Marshaller/Unmarshaller by Pointer (O + G) -> -1
        Entry {
            name: "Custom Marshaller/Unmarshaller by Pointer",
            docs: &[
                doc!(
                    Format::OpenStep,
                    "Custom Marshaller_Unmarshaller by Pointer.openstep.plist"
                ),
                doc!(
                    Format::GnuStep,
                    "Custom Marshaller_Unmarshaller by Pointer.gnustep.plist"
                ),
            ],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Integer(Integer::Signed(-1))),
            decode: decode_typed::<i64>,
            expected: || Value::Integer(Integer::Signed(-1)),
        },
        // 40. Type implementing both Text and Plist Marshaler (G) -> {a=b;}
        Entry {
            name: "Type implementing both Text and Plist Marshaler",
            docs: &[doc!(
                Format::GnuStep,
                "Type implementing both Text and Plist Marshaler.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| string_dict(&[("a", "b")])),
            decode: decode_as_value,
            expected: || string_dict(&[("a", "b")]),
        },
        // 41. Type implementing both Text and Plist Unmarshaler (G); typed decode {0}
        Entry {
            name: "Type implementing both Text and Plist Unmarshaler",
            docs: &[doc!(
                Format::GnuStep,
                "Type implementing both Text and Plist Unmarshaler.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            // Encode source is the Value-level wire shape {blah=<*I1024>;}.
            encode_value: Some(|| dict(&[("blah", Value::Integer(Integer::Signed(1024)))])),
            decode: decode_as_value,
            expected: || dict(&[("blah", Value::Integer(Integer::Signed(1024)))]),
        },
        // 42. Comments (decode-only OpenStep)
        Entry {
            name: "Comments",
            docs: &[doc!(Format::OpenStep, "Comments.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || {
                dict(&[
                    ("A", Value::String("1".to_owned())),
                    ("B", Value::String("2".to_owned())),
                    ("C", Value::String("3".to_owned())),
                    ("S", Value::String("/not/a/comment/".to_owned())),
                    ("S2", Value::String("/not*a/*comm*en/t".to_owned())),
                ])
            },
        },
        // 43. Escapes (decode-only OpenStep)
        Entry {
            name: "Escapes",
            docs: &[doc!(Format::OpenStep, "Escapes.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || {
                dict(&[
                    ("W", Value::String("w".to_owned())),
                    ("A", Value::String("\u{07}".to_owned())),
                    ("B", Value::String("\u{08}".to_owned())),
                    ("V", Value::String("\u{0b}".to_owned())),
                    ("F", Value::String("\u{0c}".to_owned())),
                    ("T", Value::String("\t".to_owned())),
                    ("R", Value::String("\r".to_owned())),
                    ("N", Value::String("\n".to_owned())),
                    ("Hex1", Value::String("\u{ab}".to_owned())),
                    ("Unicode1", Value::String("\u{ac}".to_owned())),
                    ("Unicode2", Value::String("\u{ad}".to_owned())),
                    ("Octal1", Value::String("\u{1b}".to_owned())),
                ])
            },
        },
        // 44. Empty Strings in Arrays (decode-only OpenStep) -> ["A"]
        Entry {
            name: "Empty Strings in Arrays",
            docs: &[doc!(
                Format::OpenStep,
                "Empty Strings in Arrays.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || str_array(&["A"]),
        },
        // 45. Empty Data (decode-only OpenStep) -> empty data
        Entry {
            name: "Empty Data",
            docs: &[doc!(Format::OpenStep, "Empty Data.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || Value::Data(Vec::new()),
        },
        // 46. UTF-8 with BOM (decode-only OpenStep) -> "Hello"
        Entry {
            name: "UTF-8 with BOM",
            docs: &[doc!(Format::OpenStep, "UTF-8 with BOM.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("Hello".to_owned()),
        },
        // 47. UTF-16LE with BOM (decode-only OpenStep) -> "Hello"
        Entry {
            name: "UTF-16LE with BOM",
            docs: &[doc!(Format::OpenStep, "UTF-16LE with BOM.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("Hello".to_owned()),
        },
        // 48. UTF-16BE with BOM (decode-only OpenStep) -> "Hello"
        Entry {
            name: "UTF-16BE with BOM",
            docs: &[doc!(Format::OpenStep, "UTF-16BE with BOM.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("Hello".to_owned()),
        },
        // 49. UTF-16LE without BOM (decode-only OpenStep) -> "Hello"
        Entry {
            name: "UTF-16LE without BOM",
            docs: &[doc!(
                Format::OpenStep,
                "UTF-16LE without BOM.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("Hello".to_owned()),
        },
        // 50. UTF-16BE without BOM (decode-only OpenStep) -> "Hello"
        Entry {
            name: "UTF-16BE without BOM",
            docs: &[doc!(
                Format::OpenStep,
                "UTF-16BE without BOM.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("Hello".to_owned()),
        },
        // 51. UTF-16BE with High Characters (decode-only OpenStep)
        Entry {
            name: "UTF-16BE with High Characters",
            docs: &[doc!(
                Format::OpenStep,
                "UTF-16BE with High Characters.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("Hello, 世界".to_owned()),
        },
        // 52. Legacy Strings File Format (No Dictionary) (decode-only OpenStep)
        Entry {
            name: "Legacy Strings File Format (No Dictionary)",
            docs: &[doc!(
                Format::OpenStep,
                "Legacy Strings File Format (No Dictionary).openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || string_dict(&[("Key", "Value"), ("Key2", "Value2")]),
        },
        // 53. Strings File Shortcut Format (No Values) (decode-only OpenStep)
        Entry {
            name: "Strings File Shortcut Format (No Values)",
            docs: &[doc!(
                Format::OpenStep,
                "Strings File Shortcut Format (No Values).openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || string_dict(&[("Key", "Key"), ("Key2", "Key2")]),
        },
        // 54. Various Truncated Escapes (decode-only OpenStep)
        Entry {
            name: "Various Truncated Escapes",
            docs: &[doc!(
                Format::OpenStep,
                "Various Truncated Escapes.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("\u{01}\u{02}\u{03}\u{04}\u{05}7".to_owned()),
        },
        // 55. Various Case-Insensitive Escapes (decode-only OpenStep)
        Entry {
            name: "Various Case-Insensitive Escapes",
            docs: &[doc!(
                Format::OpenStep,
                "Various Case-Insensitive Escapes.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_typed::<String>,
            expected: || Value::String("\u{ab}\u{cdef}".to_owned()),
        },
        // 56. Text data long enough to trigger reallocation (decode-only OpenStep)
        Entry {
            name: "Text data long enough to trigger implementation-specific reallocation",
            docs: &[doc!(
                Format::OpenStep,
                "Text data long enough to trigger implementation-specific reallocation.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || {
                let mut bytes = vec![0u8; 256];
                bytes.push(0x01);
                Value::Data(bytes)
            },
        },
        // 57. Empty Text Document (decode-only OpenStep) -> empty dict
        Entry {
            name: "Empty Text Document",
            docs: &[doc!(Format::OpenStep, "Empty Text Document.openstep.plist")],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || Value::Dictionary(Dictionary::new()),
        },
        // 58. Text document consisting of only whitespace (decode-only OpenStep)
        Entry {
            name: "Text document consisting of only whitespace",
            docs: &[doc!(
                Format::OpenStep,
                "Text document consisting of only whitespace.openstep.plist"
            )],
            skip_encode: &[Format::OpenStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || Value::Dictionary(Dictionary::new()),
        },
        // 59. Sized integers at size boundaries (binary; typed decode promotes)
        Entry {
            name: "Sized integers at size boundaries",
            docs: &[doc!(
                Format::Binary,
                "Sized integers at size boundaries.binary.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(sized_integers),
            decode: decode_as_value,
            expected: sized_integers,
        },
        // 60. Duplicate Dictionary Keys (decode-only ×4; last wins)
        Entry {
            name: "Duplicate Dictionary Keys",
            docs: &[
                doc!(Format::Xml, "Duplicate Dictionary Keys.xml.plist"),
                doc!(Format::Binary, "Duplicate Dictionary Keys.binary.plist"),
                doc!(Format::OpenStep, "Duplicate Dictionary Keys.openstep.plist"),
                doc!(Format::GnuStep, "Duplicate Dictionary Keys.gnustep.plist"),
            ],
            skip_encode: &[
                Format::Xml,
                Format::Binary,
                Format::OpenStep,
                Format::GnuStep,
            ],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || string_dict(&[("key", "second value")]),
        },
        // 61. GNUStep base64 data ignoring invalid chars (decode-only G)
        Entry {
            name: "GNUStep base64 data ignoring invalid chars",
            docs: &[doc!(
                Format::GnuStep,
                "GNUStep base64 data ignoring invalid chars.gnustep.plist"
            )],
            skip_encode: &[Format::GnuStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || {
                Value::Array(vec![
                    Value::Data(b"hello".to_vec()),
                    Value::Data(b"hello".to_vec()),
                ])
            },
        },
        // 62. Text document with quoted GNUstep values (decode-only G)
        Entry {
            name: "Text document with quoted GNUstep values",
            docs: &[doc!(
                Format::GnuStep,
                "Text document with quoted GNUstep values.gnustep.plist"
            )],
            skip_encode: &[Format::GnuStep],
            skip_decode: &[],
            encode_value: None,
            decode: decode_as_value,
            expected: || {
                Value::Array(vec![
                    Value::Integer(Integer::Unsigned(1_048_576)),
                    Value::Integer(Integer::Unsigned(1234)),
                    Value::Boolean(true),
                ])
            },
        },
        // 63. A struct containing a pointer to a pointer (etc) (G) -> {Intppp=<*I3>;}
        Entry {
            name: "A struct containing a pointer to a pointer (etc)",
            docs: &[doc!(
                Format::GnuStep,
                "A struct containing a pointer to a pointer (etc).gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| dict(&[("Intppp", Value::Integer(Integer::Signed(3)))])),
            decode: decode_as_value,
            expected: || dict(&[("Intppp", Value::Integer(Integer::Signed(3)))]),
        },
        // 64. Embedded fields within a nil omitempty member (G) -> {}
        Entry {
            name: "Embedded fields within a nil omitempty member",
            docs: &[doc!(
                Format::GnuStep,
                "Embedded fields within a nil omitempty member.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| Value::Dictionary(Dictionary::new())),
            decode: decode_as_value,
            expected: || Value::Dictionary(Dictionary::new()),
        },
        // 65. ...telescoping (G) -> {O=sentinel;}
        Entry {
            name: "Embedded fields within a nil omitempty member, telescoping",
            docs: &[doc!(
                Format::GnuStep,
                "Embedded fields within a nil omitempty member, telescoping.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| string_dict(&[("O", "sentinel")])),
            decode: decode_as_value,
            expected: || string_dict(&[("O", "sentinel")]),
        },
        // 66. ...telescoping, 1 (G) -> {O=sentinel;One=one;}
        Entry {
            name: "Embedded fields within a nil omitempty member, telescoping, 1",
            docs: &[doc!(
                Format::GnuStep,
                "Embedded fields within a nil omitempty member, telescoping, 1.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| string_dict(&[("O", "sentinel"), ("One", "one")])),
            decode: decode_as_value,
            expected: || string_dict(&[("O", "sentinel"), ("One", "one")]),
        },
        // 67. ...telescoping, 2 (G) -> {O=sentinel;One=one;Two=two;}
        Entry {
            name: "Embedded fields within a nil omitempty member, telescoping, 2",
            docs: &[doc!(
                Format::GnuStep,
                "Embedded fields within a nil omitempty member, telescoping, 2.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| {
                string_dict(&[("O", "sentinel"), ("One", "one"), ("Two", "two")])
            }),
            decode: decode_as_value,
            expected: || string_dict(&[("O", "sentinel"), ("One", "one"), ("Two", "two")]),
        },
        // 68. ...telescoping, non-nil-but-non-empty (G)
        Entry {
            name: "Embedded fields within a nil omitempty member, telescoping, non-nil-but-non-empty",
            docs: &[doc!(
                Format::GnuStep,
                "Embedded fields within a nil omitempty member, telescoping, non-nil-but-non-empty.gnustep.plist"
            )],
            skip_encode: &[],
            skip_decode: &[],
            encode_value: Some(|| {
                string_dict(&[("O", "sentinel"), ("One", ""), ("Three", ""), ("Two", "")])
            }),
            decode: decode_as_value,
            expected: || string_dict(&[("O", "sentinel"), ("One", ""), ("Three", ""), ("Two", "")]),
        },
    ]
}

// --- multi-line value builders ---------------------------------------------

/// Entry 7: the shadowing-resolved dictionary (spec §2.2).
fn embed_value() -> Value {
    dict(&[
        (
            "EmbedB",
            string_dict(&[
                ("FieldA", "A.B.C.A1"),
                ("FieldA2", "A.B.C.A2"),
                ("FieldB", "A.B.B"),
                ("FieldC", "A.B.C.C"),
            ]),
        ),
        ("FieldA", Value::String("A.A".to_owned())),
        ("FieldA2", Value::String(String::new())),
        ("FieldB", Value::String("A.C.B".to_owned())),
        ("FieldC", Value::String("A.C.C".to_owned())),
    ])
}

/// Entry 19: the mega-map. f32 32.0 is narrow (binary tag 0x22).
fn map_all_variations() -> Value {
    dict(&[
        (
            "intarray",
            Value::Array(vec![
                Value::Integer(Integer::Signed(1)),
                Value::Integer(Integer::Signed(8)),
                Value::Integer(Integer::Signed(16)),
                Value::Integer(Integer::Signed(32)),
                Value::Integer(Integer::Signed(64)),
                Value::Integer(Integer::Unsigned(2)),
                Value::Integer(Integer::Unsigned(9)),
                Value::Integer(Integer::Unsigned(17)),
                Value::Integer(Integer::Unsigned(33)),
                Value::Integer(Integer::Unsigned(65)),
            ]),
        ),
        (
            "floats",
            Value::Array(vec![
                Value::Real(Real::from(32.0_f32)),
                Value::Real(Real::from(64.0_f64)),
            ]),
        ),
        (
            "booleans",
            Value::Array(vec![Value::Boolean(true), Value::Boolean(false)]),
        ),
        ("strings", str_array(&["Hello, ASCII", "Hello, 世界"])),
        ("data", Value::Data(vec![1, 2, 3, 4])),
        ("date", Value::Date(date_2013())),
    ])
}

/// Entry 21: the SparseBundleHeader value.
fn sparse_bundle() -> Value {
    dict(&[
        (
            "CFBundleInfoDictionaryVersion",
            Value::String("6.0".to_owned()),
        ),
        ("band-size", Value::Integer(Integer::Unsigned(8_388_608))),
        (
            "bundle-backingstore-version",
            Value::Integer(Integer::Signed(1)),
        ),
        (
            "diskimage-bundle-type",
            Value::String("com.apple.diskimage.sparsebundle".to_owned()),
        ),
        ("size", Value::Integer(Integer::Unsigned(4_398_046_511_104))),
    ])
}

/// Entry 31: the dedup-stressing array. f32 and f64 of the same magnitude stay
/// structurally distinct in the binary object table; the projection compares
/// numerically (Real ignores `wide`).
fn duplicated_values() -> Value {
    Value::Array(vec![
        Value::String("Hello".to_owned()),
        Value::Real(Real::from(32.0_f32)),
        Value::Real(Real::from(32.0_f64)),
        Value::Data(b"data".to_vec()),
        Value::Real(Real::from(64.0_f32)),
        Value::Real(Real::from(64.0_f64)),
        Value::Integer(Integer::Unsigned(100)),
        Value::Real(Real::from(32.0_f32)),
        Value::Date(date_2013()),
        Value::Real(Real::from(32.0_f64)),
        Value::Real(Real::from(64.0_f32)),
        Value::Real(Real::from(64.0_f64)),
        Value::String("Hello".to_owned()),
        Value::Data(b"data".to_vec()),
        Value::Integer(Integer::Unsigned(100)),
        Value::Date(date_2013()),
    ])
}

/// Entry 32: the funny-characters map (byte-wise key order).
fn funny_characters() -> Value {
    string_dict(&[
        ("\u{07}", "\u{08}"),
        ("\t\r", "\n"),
        ("\u{0b}", "\u{0c}"),
        ("\\", "\""),
        ("\u{c8}", "wat"),
        ("\u{100}", "hundred"),
    ])
}

/// Entry 12: unsigned integers spanning each binary width tier.
fn unsigned_increasing() -> Value {
    uint_array(&[
        0xff,
        0x0fff,
        0xffff,
        0x000f_ffff,
        0x00ff_ffff,
        0x0fff_ffff,
        0xffff_ffff,
        0x7fff_ffff_ffff_ffff,
        0xdead_beef_face_cafe,
    ])
}

/// Entry 35: the UID array.
fn cf_uids() -> Value {
    Value::Array(
        [0xff_u64, 0xffff, 0x00ff_ffff, 0xffff_ffff, 0x00ff_ffff_ffff]
            .into_iter()
            .map(|n| Value::Uid(Uid::from(n)))
            .collect(),
    )
}

/// Entry 59: signed/unsigned boundary integers. The typed decode promotes
/// to int64/uint64; numeric `Integer` equality absorbs the signedness split.
fn sized_integers() -> Value {
    Value::Array(vec![
        Value::Integer(Integer::Signed(-128)),
        Value::Integer(Integer::Signed(127)),
        Value::Integer(Integer::Signed(-32768)),
        Value::Integer(Integer::Signed(32767)),
        Value::Integer(Integer::Signed(-2_147_483_648)),
        Value::Integer(Integer::Signed(2_147_483_647)),
        Value::Integer(Integer::Signed(i64::MIN)),
        Value::Integer(Integer::Signed(i64::MAX)),
    ])
}
