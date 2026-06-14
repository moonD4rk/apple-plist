//! Hand-written `Serialize`/`Deserialize` impls through the serde bridge for
//! the custom-marshaler cases. The classic value- and text-marshaling hooks
//! (both directions) collapse into one `Serialize` and one `Deserialize` impl
//! per type (spec 05 §8); these tests pin the resulting wire shapes against the
//! goldens.

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test assertions: an unwrap or panic firing is the test failing"
)]

use std::collections::BTreeMap;
use std::fmt;

use apple_plist::{Date, Format, Value, from_slice, to_value, to_vec};
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::macros::datetime;

const PREAMBLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

fn xml_doc(body: &str) -> String {
    format!("{PREAMBLE}<plist version=\"1.0\">{body}</plist>")
}

// --- Minimal standard-alphabet, padded base64 (RFC 4648), to keep this test ---
// --- dependency-free; matches the standard base64 encoding (spec 05 §8.1). ---

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_symbol(sextet: u32) -> char {
    char::from(
        B64_ALPHABET
            .iter()
            .copied()
            .nth(sextet as usize)
            .unwrap_or(b'='),
    )
}

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let mut bytes = chunk.iter().copied();
        let b0 = u32::from(bytes.next().unwrap_or(0));
        let b1 = u32::from(bytes.next().unwrap_or(0));
        let b2 = u32::from(bytes.next().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(b64_symbol((triple >> 18) & 0x3f));
        out.push(b64_symbol((triple >> 12) & 0x3f));
        out.push(if chunk.len() > 1 {
            b64_symbol((triple >> 6) & 0x3f)
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            b64_symbol(triple & 0x3f)
        } else {
            '='
        });
    }
    out
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    fn sextet(c: u8) -> Result<u32, String> {
        u32::try_from(
            B64_ALPHABET
                .iter()
                .position(|&a| a == c)
                .ok_or_else(|| format!("invalid base64 character: {c:#x}"))?,
        )
        .map_err(|err| err.to_string())
    }
    let stripped: Vec<u8> = input.bytes().filter(|b| *b != b'=').collect();
    let mut out = Vec::new();
    for chunk in stripped.chunks(4) {
        let mut acc = 0_u32;
        for &c in chunk {
            acc = (acc << 6) | sextet(c)?;
        }
        let bits = chunk.len() * 6;
        let bytes = bits / 8;
        acc <<= u32::try_from(32 - bits).map_err(|err| err.to_string())?;
        out.extend(acc.to_be_bytes().into_iter().take(bytes));
    }
    Ok(out)
}

// --- Base64String: the canonical custom marshaler --------------------------

#[derive(PartialEq, Eq, Debug)]
struct Base64String(String);

impl Serialize for Base64String {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64_encode(self.0.as_bytes()))
    }
}

impl<'de> Deserialize<'de> for Base64String {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let bytes = base64_decode(&encoded).map_err(serde::de::Error::custom)?;
        String::from_utf8(bytes)
            .map(Base64String)
            .map_err(serde::de::Error::custom)
    }
}

#[test]
fn base64_string_golden() {
    // "Dustin" -> "RHVzdGlu" (OpenStep), and the round-trip restores "Dustin".
    let value = Base64String("Dustin".into());
    let document = to_vec(&value, Format::OpenStep).unwrap();
    assert_eq!(document, b"RHVzdGlu");

    let decoded: Base64String = from_slice(&document).unwrap();
    assert_eq!(decoded, Base64String("Dustin".into()));
}

#[test]
fn base64_string_round_trips_every_format() {
    let value = Base64String("Dustin".into());
    for format in [
        Format::Xml,
        Format::Binary,
        Format::OpenStep,
        Format::GnuStep,
    ] {
        let document = to_vec(&value, format).unwrap();
        let decoded: Base64String = from_slice(&document).unwrap();
        assert_eq!(decoded, value, "round-trip in {format}");
    }
}

// --- ArrayThatSerializesAsOneObject (entry 38) -------------------------------

#[derive(PartialEq, Eq, Debug)]
struct ArrayAsOneObject(Vec<u64>);

impl Serialize for ArrayAsOneObject {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // len == 1 -> a bare scalar; otherwise a sequence.
        match self.0.as_slice() {
            [single] => serializer.serialize_u64(*single),
            many => many.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ArrayAsOneObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Try a single u64 first, then a slice: one multi-method visitor
        // expresses both arms.
        struct OneOrMany;
        impl<'de> Visitor<'de> for OneOrMany {
            type Value = ArrayAsOneObject;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an unsigned integer or a sequence of them")
            }
            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(ArrayAsOneObject(vec![value]))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(item) = seq.next_element::<u64>()? {
                    values.push(item);
                }
                Ok(ArrayAsOneObject(values))
            }
        }
        deserializer.deserialize_any(OneOrMany)
    }
}

#[test]
fn array_as_one_object_gnustep_golden() {
    let value = vec![
        ArrayAsOneObject(vec![100]),
        ArrayAsOneObject(vec![2, 4, 6, 8]),
    ];
    let document = to_vec(&value, Format::GnuStep).unwrap();
    assert_eq!(
        document,
        b"(<*I100>,(<*I2>,<*I4>,<*I6>,<*I8>,),)".as_slice()
    );

    let decoded: Vec<ArrayAsOneObject> = from_slice(&document).unwrap();
    assert_eq!(decoded, value);
}

// --- PlistMarshalingBoolByPointer (entry 39) ---------------------------------

#[derive(PartialEq, Eq, Debug)]
struct PlistBool(bool);

impl Serialize for PlistBool {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // true -> -1, false -> -2.
        serializer.serialize_i64(if self.0 { -1 } else { -2 })
    }
}

impl<'de> Deserialize<'de> for PlistBool {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = i64::deserialize(deserializer)?;
        Ok(Self(raw == -1))
    }
}

#[test]
fn plist_bool_by_pointer_goldens() {
    let value = PlistBool(true);
    assert_eq!(to_vec(&value, Format::OpenStep).unwrap(), b"-1");
    assert_eq!(to_vec(&value, Format::GnuStep).unwrap(), b"<*I-1>");
}

#[test]
fn plist_bool_decodes_lax_string_through_custom_impl() {
    // The OpenStep document is the bare string "-1"; through the lax ladder the
    // custom Deserialize coerces it to i64 == -1, hence true (spec 05 §8 / RFC
    // 0003 §8).
    let decoded: PlistBool = from_slice(b"-1").unwrap();
    assert_eq!(decoded, PlistBool(true));
}

// --- BothMarshaler (entry 40) ------------------------------------------------

#[derive(Default)]
struct BothMarshaler;

impl Serialize for BothMarshaler {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // When both a value marshaler (-> {a=b}) and a text marshaler exist,
        // the value marshaler wins; here a single impl emits that wire shape.
        let map: BTreeMap<&str, &str> = BTreeMap::from([("a", "b")]);
        map.serialize(serializer)
    }
}

#[test]
fn both_marshaler_gnustep_golden() {
    assert_eq!(to_vec(&BothMarshaler, Format::GnuStep).unwrap(), b"{a=b;}");
}

// --- BothUnmarshaler (entry 41) ----------------------------------------------

#[derive(PartialEq, Eq, Debug, Default)]
struct BothUnmarshaler {
    blah: i64,
}

impl<'de> Deserialize<'de> for BothUnmarshaler {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A no-op deserialize must still consume the node; IgnoredAny consumes
        // any value and the field stays at its default.
        let _consumed: serde::de::IgnoredAny = Deserialize::deserialize(deserializer)?;
        Ok(Self::default())
    }
}

#[test]
fn both_unmarshaler_ignores_content() {
    // Doc {blah=<*I1024>;} decodes to {blah: 0}.
    let decoded: BothUnmarshaler = from_slice(b"{blah=<*I1024>;}").unwrap();
    assert_eq!(decoded, BothUnmarshaler { blah: 0 });
}

// --- Interface marshal / interface-field marshal -----------------------------

struct Cat;

impl Serialize for Cat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The custom impl returns the substitute "cat".
        serializer.serialize_str("cat")
    }
}

// A struct field of the custom type — the custom marshaler held in a field;
// same wire assertion as the bare value.
#[derive(Serialize)]
struct CatHolder {
    #[serde(rename = "C")]
    c: Cat,
}

#[test]
fn custom_serialize_substitute() {
    let document = to_vec(&Cat, Format::Xml).unwrap();
    assert_eq!(
        document,
        xml_doc("<string>cat</string>").as_bytes(),
        "Cat marshals to its substitute string"
    );

    let document = to_vec(&CatHolder { c: Cat }, Format::Xml).unwrap();
    assert_eq!(
        document,
        xml_doc("<dict><key>C</key><string>cat</string></dict>").as_bytes()
    );
}

// --- Marshaling a time-typed interface field ---------------------------------

#[test]
fn to_value_date_field() {
    // A struct with a Date field must project to a dictionary whose value is a
    // Value::Date; dates are special-cased before text marshaling.
    #[derive(Serialize)]
    struct Holder {
        #[serde(rename = "C")]
        c: Date,
    }
    let date = Date::from(std::time::SystemTime::from(
        datetime!(2013-11-27 00:34:00 UTC),
    ));
    let projected = to_value(&Holder { c: date }).unwrap();
    let Value::Dictionary(dict) = &projected else {
        panic!("expected a dictionary, got {projected:?}");
    };
    assert!(
        matches!(dict.get("C"), Some(Value::Date(_))),
        "inner value must be a date"
    );
}

// --- Heterogeneous interface-slice marshal -----------------------------------

#[test]
fn heterogeneous_slice() {
    // A Vec<Value> mixing dict/string/int/bool marshals to non-empty bytes.
    let slice = vec![
        Value::from_iter([("Name".to_owned(), Value::from("dog"))]),
        Value::from("a string"),
        Value::from(1_i64),
        Value::from(true),
    ];
    let document = to_vec(&slice, Format::Xml).unwrap();
    assert!(
        !document.is_empty(),
        "heterogeneous slice marshals to non-empty data"
    );

    let decoded: Vec<Value> = from_slice(&document).unwrap();
    assert_eq!(decoded.len(), 4);
}

// --- Custom date unmarshal ---------------------------------------------------

struct CustomDate;

impl<'de> Deserialize<'de> for CustomDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A no-op deserialize must still consume the node — even when that node
        // is a Date.
        let _consumed: serde::de::IgnoredAny = Deserialize::deserialize(deserializer)?;
        Ok(Self)
    }
}

#[test]
fn custom_impl_ignores_payload() {
    // Fractional seconds in the RFC3339 date must be accepted at parse time
    // (2003-02-03T09:00:00.00Z).
    let document = xml_doc("<date>2003-02-03T09:00:00.00Z</date>");
    let decoded: Result<CustomDate, _> = from_slice(document.as_bytes());
    assert!(
        decoded.is_ok(),
        "a no-op custom Deserialize must consume a Date value"
    );
}

// --- Invalid marshal (partial) -----------------------------------------------

#[test]
fn unmarshalable_roots() {
    // A None root -> NoRootElement.
    let none_root = to_vec(&Option::<i32>::None, Format::OpenStep);
    assert!(matches!(none_root, Err(apple_plist::Error::NoRootElement)));

    // Integer-keyed map -> error.
    let int_keyed: BTreeMap<i32, String> = BTreeMap::from([(1, "hi".into())]);
    assert!(to_vec(&int_keyed, Format::OpenStep).is_err());
    // Unserializable types (closures, channels) have no counterpart here —
    // they are a compile error, not a runtime panic (spec 07 §7).
}

// --- Invalid map-key-type unmarshal ------------------------------------------

#[test]
fn int_keyed_map_decode_errors() {
    // The public contract: decoding a dictionary into a map with a non-string
    // key type errors, never panics.
    let document = xml_doc("<dict><key>1</key><string>first</string></dict>");
    let decoded: Result<BTreeMap<i32, String>, _> = from_slice(document.as_bytes());
    assert!(decoded.is_err(), "integer-keyed map decode must error");
}

// --- Valid but aliased map-key-type unmarshal --------------------------------

#[test]
fn newtype_string_key_map_decodes() {
    // A newtype over String is a valid (string-kinded) map key.
    #[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
    struct SortaString(String);

    let document = xml_doc(
        "<dict><key>1</key><string>first</string><key>2</key><string>second</string></dict>",
    );
    let decoded: BTreeMap<SortaString, String> = from_slice(document.as_bytes()).unwrap();
    assert_eq!(
        decoded.get(&SortaString("1".into())),
        Some(&"first".to_owned())
    );
    assert_eq!(
        decoded.get(&SortaString("2".into())),
        Some(&"second".to_owned())
    );
}
