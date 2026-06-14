//! Property-based round-trip and determinism tests (RFC 0005 §3).
//!
//! A recursion-bounded [`arb_value`] strategy generates every `Value` case;
//! the properties assert the codec contracts that must hold for *all* inputs:
//! XML and binary are lossless round-trips, every encoder is deterministic,
//! GNUStep round-trips everything the text grammar can express, and `detect`
//! reports the format the decode ladder actually reaches.
//!
//! Exclusions reflect each codec's own caveats and the implementation
//! contract. Each is a real format limitation, not a bug, so the generator
//! (or a per-property `prop_assume!`) keeps inputs inside the lossless domain:
//!
//! - **NaN reals** never appear: `Real`'s equality is non-reflexive on NaN
//!   (IEEE semantics), so a NaN leaf would
//!   make `decode(encode(v)) == v` fail by construction (R2). Infinities are
//!   kept — they round-trip.
//! - **Dates** are whole-second and within the binary-f64-safe band
//!   `|apple_epoch_seconds| <= 2^31` ([`MIN_DATE_SECONDS`]..=[`MAX_DATE_SECONDS`]):
//!   binary stores Apple-epoch seconds as `f64`, so only this band recovers a
//!   whole second exactly through the nanosecond split; XML and the text layout
//!   additionally drop sub-second precision, hence whole seconds only (R13).
//! - **Strings** are restricted to the text-safe scalar set: the XML-1.0
//!   illegal C0 controls (all but tab/LF/CR) and the U+FFFE/U+FFFF
//!   noncharacters cannot survive an XML round-trip; `;` and `/` are emitted
//!   bare by the OpenStep/GNUStep generator (its quote tables omit
//!   both), so a `;` terminator or a `//` / `/*` comment-start would not read
//!   back. All are excluded from the generator; binary keeps every byte.
//! - **Astral strings** (any `char >= U+10000`) and **empty strings as array
//!   elements** are excluded from the GNUStep round-trip only: astral scalars
//!   are lossy through the 4-hex `\U` escape (RFC 0004 §4.4), and empty array
//!   elements vanish in the text grammar (`(A,,,"",)` → `["A"]`, spec 07 entry
//!   44). XML and binary keep both.
//! - **`CF$UID` keys** are never generated: a single-entry dictionary keyed
//!   exactly `CF$UID` with an integer value decodes back as a `Uid`, not a
//!   dictionary, in every text/XML format (R19) — a structural collision, not
//!   a codec bug.

#![expect(
    clippy::expect_used,
    reason = "tests assert via expect; a failure is the test failing"
)]

use std::time::{Duration, SystemTime};

use apple_plist::{
    Date, Dictionary, Format, Integer, Real, Uid, Value, detect, from_slice, to_vec,
};
use proptest::prelude::*;

/// Recursion depth well under the parser's 128 cap (RFC 0005 §3).
const MAX_DEPTH: u32 = 6;
/// Upper bound on the elements of any one generated collection.
const MAX_COLLECTION: u32 = 6;
/// proptest cases per property — CI-friendly, overridable via `PROPTEST_CASES`.
const CASES: u32 = 256;

/// Apple epoch (2001-01-01T00:00:00Z) as Unix seconds, mirroring the binary
/// codec's offset.
const APPLE_EPOCH_UNIX_SECONDS: i64 = 978_307_200;
/// Half-width of the binary-f64-safe whole-second band: at `2^31` Apple-epoch
/// seconds the `f64` still recovers every whole second exactly through the
/// nanosecond round-trip; the failures begin at `2^32` (verified empirically).
const DATE_HALF_BAND: i64 = 1 << 31;
/// Smallest whole-second Unix timestamp the date strategy draws (≈ year 1932).
const MIN_DATE_SECONDS: i64 = APPLE_EPOCH_UNIX_SECONDS - DATE_HALF_BAND;
/// Largest whole-second Unix timestamp the date strategy draws (≈ year 2099).
const MAX_DATE_SECONDS: i64 = APPLE_EPOCH_UNIX_SECONDS + DATE_HALF_BAND;

fn date_from_whole_seconds(secs: i64) -> Date {
    let instant = if secs >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    };
    Date::from(instant)
}

/// Whether a scalar survives a round-trip through *every* text/XML format.
///
/// Two unrelated constraints combine:
///
/// - **XML 1.0**: the C0 controls other than tab/LF/CR are illegal, and
///   U+FFFE/U+FFFF are noncharacters the tokenizer rejects.
/// - **OpenStep/GNUStep generator quote tables** (`osQuotable`/`gsQuotable`):
///   neither table marks `;` or `/` as
///   quotable, so the text generator emits them bare — a bare `;` is a
///   statement terminator and a bare `//` / `/*` starts a comment, neither of
///   which the parser reads back. This is an inherent generator
///   asymmetry, so the generator simply avoids those bytes.
const fn is_text_safe(c: char) -> bool {
    !matches!(c as u32,
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0xFFFE | 0xFFFF | 0x2F | 0x3B)
}

/// A mix of ASCII, BMP, and (optionally) astral scalars so the string codecs
/// are exercised across all three escape regimes, restricted to the text-safe
/// character set. Astral planes are dropped when `allow_astral` is false (the
/// GNUStep generator, whose 4-hex `\U` escape cannot carry them).
fn arb_string(allow_astral: bool) -> BoxedStrategy<String> {
    let mut arms = vec![
        Just(String::new()).boxed(),
        "[ -~]{1,8}".boxed(),       // printable ASCII
        "[\t\n\r -~]{1,6}".boxed(), // ASCII + whitespace
        prop::collection::vec(0x80u32..=0xff, 1..4)
            .prop_map(codepoints)
            .boxed(), // Latin-1
        prop::collection::vec(0x100u32..=0xfffd, 1..4)
            .prop_map(codepoints)
            .boxed(), // BMP
    ];
    if allow_astral {
        arms.push(
            prop::collection::vec(0x1_0000u32..=0x10_ffff, 1..3)
                .prop_map(codepoints)
                .boxed(),
        );
    }
    prop::strategy::Union::new(arms)
        .prop_map(|s| s.chars().filter(|c| is_text_safe(*c)).collect::<String>())
        .boxed()
}

fn codepoints(codes: Vec<u32>) -> String {
    codes.into_iter().filter_map(char::from_u32).collect()
}

fn arb_integer() -> impl Strategy<Value = Integer> {
    prop_oneof![
        any::<i64>().prop_map(Integer::Signed),
        any::<u64>().prop_map(Integer::Unsigned),
    ]
}

/// Finite reals and infinities (NaN excluded). Both widths are generated so
/// the binary `0x22`/`0x23` channel and its dedup dimension are covered.
fn arb_real() -> impl Strategy<Value = Real> {
    prop_oneof![
        any::<f32>()
            .prop_filter("no NaN", |f| !f.is_nan())
            .prop_map(Real::from),
        any::<f64>()
            .prop_filter("no NaN", |f| !f.is_nan())
            .prop_map(Real::from),
    ]
}

fn arb_date() -> impl Strategy<Value = Date> {
    (MIN_DATE_SECONDS..=MAX_DATE_SECONDS).prop_map(date_from_whole_seconds)
}

/// Dictionary keys: any generated string except the `CF$UID` sentinel, whose
/// single-entry-integer shape aliases to a `Uid` on decode (R19).
fn arb_key(allow_astral: bool) -> BoxedStrategy<String> {
    arb_string(allow_astral)
        .prop_filter("CF$UID aliases to Uid", |s| s != "CF$UID")
        .boxed()
}

fn arb_leaf(allow_astral: bool) -> BoxedStrategy<Value> {
    prop_oneof![
        arb_string(allow_astral).prop_map(Value::String),
        arb_integer().prop_map(Value::Integer),
        arb_real().prop_map(Value::Real),
        any::<bool>().prop_map(Value::Boolean),
        prop::collection::vec(any::<u8>(), 0..8).prop_map(Value::Data),
        arb_date().prop_map(Value::Date),
        any::<u64>().prop_map(|n| Value::Uid(Uid::from(n))),
    ]
    .boxed()
}

/// The general generator: every value case, astral strings included. Used by
/// the XML, binary, determinism, and detection properties.
fn arb_value() -> impl Strategy<Value = Value> {
    let len = MAX_COLLECTION as usize;
    arb_leaf(true).prop_recursive(MAX_DEPTH, 64, MAX_COLLECTION, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..len).prop_map(Value::Array),
            prop::collection::btree_map(arb_key(true), inner, 0..len)
                .prop_map(|m| Value::Dictionary(m.into_iter().collect())),
        ]
    })
}

/// The GNUStep-lossless generator: no astral strings anywhere, and no empty
/// string sitting directly inside an array (those two shapes are lossy through
/// the text grammar — see the module docs). Built directly so the GNUStep
/// round-trip property rejects nothing.
fn arb_gnustep_value() -> impl Strategy<Value = Value> {
    let len = MAX_COLLECTION as usize;
    arb_leaf(false).prop_recursive(MAX_DEPTH, 64, MAX_COLLECTION, move |inner| {
        let array_element = inner.clone().prop_filter(
            "empty array-element strings vanish",
            |item| !matches!(item, Value::String(s) if s.is_empty()),
        );
        prop_oneof![
            prop::collection::vec(array_element, 0..len).prop_map(Value::Array),
            prop::collection::btree_map(arb_key(false), inner, 0..len)
                .prop_map(|m| Value::Dictionary(m.into_iter().collect())),
        ]
    })
}

/// Whether the GNUStep text grammar can round-trip this tree losslessly. It
/// cannot when a string carries an astral `char` (lossy 4-hex `\U`) or when an
/// empty string sits directly inside an array (empty array elements vanish).
fn gnustep_lossless(value: &Value) -> bool {
    match value {
        Value::String(s) => !s.chars().any(|c| c as u32 >= 0x1_0000),
        Value::Array(items) => items.iter().all(|item| {
            !matches!(item, Value::String(s) if s.is_empty()) && gnustep_lossless(item)
        }),
        Value::Dictionary(map) => {
            map.keys().all(|k| !k.chars().any(|c| c as u32 >= 0x1_0000))
                && map.values().all(gnustep_lossless)
        }
        _ => true,
    }
}

/// Whether the GNUStep encoding of this tree carries a typed `<*…>` literal,
/// which flips the text parser's verdict from OpenStep to GNUStep and sticks.
/// Integers, reals, booleans, and dates emit `<*I/R/B/D…>`; a `Uid` lowers to
/// `{CF$UID=<*I…>;}`, so it counts too. Data emits bare `<hex>` (untyped) in
/// both dialects, and strings/empty containers carry no literal.
fn gnustep_emits_typed_literal(value: &Value) -> bool {
    match value {
        Value::Integer(_) | Value::Real(_) | Value::Boolean(_) | Value::Date(_) | Value::Uid(_) => {
            true
        }
        Value::Array(items) => items.iter().any(gnustep_emits_typed_literal),
        Value::Dictionary(map) => map.values().any(gnustep_emits_typed_literal),
        _ => false,
    }
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: CASES,
        // .proptest-regressions is not gitignore-whitelisted; never persist.
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// (1) XML is a lossless round-trip for every value shape.
    #[test]
    fn xml_round_trips(value in arb_value()) {
        let bytes = to_vec(&value, Format::Xml).expect("xml encode");
        let back: Value = from_slice(&bytes).expect("xml decode");
        prop_assert_eq!(&back, &value, "xml round-trip");
    }

    /// (2) Binary is a lossless round-trip for every value shape, including
    /// narrow/wide real widths and whole-second dates.
    #[test]
    fn binary_round_trips(value in arb_value()) {
        let bytes = to_vec(&value, Format::Binary).expect("binary encode");
        let back: Value = from_slice(&bytes).expect("binary decode");
        prop_assert_eq!(&back, &value, "binary round-trip");
    }

    /// (3) Every encoder is deterministic: the same value encodes to identical
    /// bytes on repeated calls, in all four formats.
    #[test]
    fn encoding_is_deterministic(value in arb_value()) {
        for format in [Format::Xml, Format::Binary, Format::OpenStep, Format::GnuStep] {
            let first = to_vec(&value, format).expect("first encode");
            let second = to_vec(&value, format).expect("second encode");
            prop_assert_eq!(&first, &second, "{:?} encoding is not deterministic", format);
        }
    }

    /// (4) GNUStep round-trips every value shape the text grammar can express.
    /// The dedicated [`arb_gnustep_value`] strategy already excludes the lossy
    /// shapes (astral strings, empty array-element strings); the explicit
    /// `gnustep_lossless` guard documents and re-checks that invariant.
    #[test]
    fn gnustep_round_trips(value in arb_gnustep_value()) {
        prop_assert!(gnustep_lossless(&value), "generator produced a lossy shape");
        let bytes = to_vec(&value, Format::GnuStep).expect("gnustep encode");
        let back: Value = from_slice(&bytes).expect("gnustep decode");
        prop_assert_eq!(&back, &value, "gnustep round-trip");
    }

    /// (5) `detect` over every encoded buffer reports the format the decode
    /// ladder actually reaches. XML and binary are self-identifying; an
    /// OpenStep buffer always detects as OpenStep; a GNUStep buffer detects as
    /// GNUStep exactly when it carries a typed `<*…>` literal, otherwise the
    /// sticky-flip never fires and the ladder reports OpenStep.
    #[test]
    fn detect_matches_the_encoded_format(value in arb_value()) {
        let xml = to_vec(&value, Format::Xml).expect("xml encode");
        prop_assert_eq!(detect(&xml), Some(Format::Xml), "xml detect");

        let binary = to_vec(&value, Format::Binary).expect("binary encode");
        prop_assert_eq!(detect(&binary), Some(Format::Binary), "binary detect");

        let openstep = to_vec(&value, Format::OpenStep).expect("openstep encode");
        prop_assert_eq!(detect(&openstep), Some(Format::OpenStep), "openstep detect");

        let gnustep = to_vec(&value, Format::GnuStep).expect("gnustep encode");
        let expected = if gnustep_emits_typed_literal(&value) {
            Format::GnuStep
        } else {
            Format::OpenStep
        };
        prop_assert_eq!(detect(&gnustep), Some(expected), "gnustep detect");
    }
}

#[test]
fn date_band_round_trips_at_its_edges() {
    // The constants the date strategy draws from must themselves survive
    // XML/binary/GNUStep, guarding the band against an off-by-one widening.
    for secs in [MIN_DATE_SECONDS, 0, 1_385_512_440, MAX_DATE_SECONDS] {
        let value = Value::Date(date_from_whole_seconds(secs));
        for format in [Format::Xml, Format::Binary, Format::GnuStep] {
            let bytes = to_vec(&value, format).expect("encode");
            let back: Value = from_slice(&bytes).expect("decode");
            assert_eq!(back, value, "date {secs} via {format:?}");
        }
    }
}

#[test]
fn gnustep_lossless_excludes_known_lossy_shapes() {
    // Astral string, empty array element: lossy. BMP string, empty dict value,
    // non-empty array element: lossless.
    assert!(!gnustep_lossless(&Value::String("\u{1F600}".into())));
    assert!(!gnustep_lossless(&Value::Array(vec![Value::String(
        String::new()
    )])));
    assert!(gnustep_lossless(&Value::String("caf\u{e9}".into())));
    assert!(gnustep_lossless(&Value::Array(vec![Value::String(
        "x".into()
    )])));

    let mut empty_value = Dictionary::new();
    drop(empty_value.insert("k".to_owned(), Value::String(String::new())));
    assert!(gnustep_lossless(&Value::Dictionary(empty_value)));
}

#[test]
fn typed_literal_predicate_tracks_the_generator() {
    // A buried integer flips GNUStep detection; a string/data-only tree does not.
    let mut buried = Dictionary::new();
    drop(buried.insert(
        "k".to_owned(),
        Value::Array(vec![
            Value::String("s".into()),
            Value::Integer(Integer::Signed(1)),
        ]),
    ));
    assert!(gnustep_emits_typed_literal(&Value::Dictionary(buried)));

    let untyped = Value::Array(vec![Value::String("a".into()), Value::Data(vec![1, 2])]);
    assert!(!gnustep_emits_typed_literal(&untyped));
}
