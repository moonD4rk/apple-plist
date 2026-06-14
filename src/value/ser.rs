//! The serde bridge from Rust values to [`Value`] trees: the
//! serialization-side kind switch.

#[cfg(test)]
use std::collections::BTreeMap;

use serde::ser::{
    Impossible, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant, Serializer,
};
use time::{Month, PrimitiveDateTime, Time};

use crate::date::Date;
use crate::error::{Error, Result};
use crate::uid::Uid;
use crate::value::{Dictionary, Integer, Real, Value};

/// Sentinel newtype name carrying a [`Date`] through serde as an RFC 3339
/// string payload. Never re-exported.
pub(crate) const DATE_NEWTYPE: &str = "$__plist_private_Date";

/// Sentinel newtype name carrying a [`Uid`] through serde as a `u64` payload.
/// Never re-exported.
pub(crate) const UID_NEWTYPE: &str = "$__plist_private_Uid";

const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// Converts any [`Serialize`] value into an owned [`Value`] tree.
///
/// `None`, `()`, and unit structs cannot be represented (property lists have
/// no null): inside containers they are dropped, at the root they error.
///
/// # Errors
///
/// Returns [`Error::NoRootElement`] when the root value serializes to null,
/// [`Error::UnknownType`] for non-string map keys and out-of-range 128-bit
/// integers, and [`Error::Message`] for errors raised by custom `Serialize`
/// implementations. [`Error::NullNotRepresentable`] is internal and never
/// escapes this function.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
///
/// use apple_plist::Value;
///
/// let tree = apple_plist::to_value(&BTreeMap::from([("answer", 42)]))?;
/// let dict = tree.as_dictionary().expect("maps become dictionaries");
/// assert_eq!(
///     dict.get("answer").and_then(Value::as_integer),
///     Some(42.into())
/// );
/// # Ok::<(), apple_plist::Error>(())
/// ```
pub fn to_value<T>(value: &T) -> Result<Value>
where
    T: Serialize + ?Sized,
{
    match value.serialize(ValueSerializer) {
        Err(Error::NullNotRepresentable) => Err(Error::NoRootElement),
        result => result,
    }
}

/// Formats the [`Date`] sentinel payload: RFC 3339 in UTC with a `Z` suffix
/// and up to nine fractional digits (trailing zeros trimmed), enough to
/// round-trip the nanosecond model losslessly.
pub(crate) fn format_sentinel_date(date: Date) -> String {
    let mut formatted = date.format_rfc3339();
    let (_, nanos) = date.unix_parts();
    if nanos != 0 && formatted.ends_with('Z') {
        formatted.truncate(formatted.len() - 1);
        let fraction = format!("{nanos:09}");
        formatted.push('.');
        formatted.push_str(fraction.trim_end_matches('0'));
        formatted.push('Z');
    }
    formatted
}

/// Parses the [`Date`] sentinel payload: the RFC 3339 grammar, extended
/// with the negative-year form [`format_sentinel_date`] emits for dates
/// before year zero (which plain RFC 3339 cannot express).
pub(crate) fn parse_sentinel_date(payload: &str) -> Option<Date> {
    payload
        .strip_prefix('-')
        .map_or_else(|| Date::parse_rfc3339(payload), parse_negative_year_date)
}

/// The original `f32` behind a narrow [`Real`]. Narrow reals always hold
/// `f32`-widened values, so the cast is exact.
pub(crate) const fn narrow_f32(real: Real) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "narrow reals are constructed from f32, so the round-trip is exact"
    )]
    let narrow = real.value() as f32;
    narrow
}

fn ascii_number(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |acc, &byte| {
        byte.is_ascii_digit()
            .then(|| acc * 10 + u32::from(byte - b'0'))
    })
}

fn byte_at(bytes: &[u8], index: usize, expected: u8) -> Option<()> {
    (bytes.get(index) == Some(&expected)).then_some(())
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z` with the year negated — only this
/// crate's own formatter produces the shape, so the grammar is fixed.
fn parse_negative_year_date(rest: &str) -> Option<Date> {
    let bytes = rest.as_bytes();
    let year = ascii_number(bytes.get(0..4)?)?;
    byte_at(bytes, 4, b'-')?;
    let month = ascii_number(bytes.get(5..7)?)?;
    byte_at(bytes, 7, b'-')?;
    let day = ascii_number(bytes.get(8..10)?)?;
    byte_at(bytes, 10, b'T')?;
    let hour = ascii_number(bytes.get(11..13)?)?;
    byte_at(bytes, 13, b':')?;
    let minute = ascii_number(bytes.get(14..16)?)?;
    byte_at(bytes, 16, b':')?;
    let second = ascii_number(bytes.get(17..19)?)?;
    let nanos = match bytes.get(19) {
        Some(b'Z') if bytes.len() == 20 => 0,
        Some(b'.') if bytes.last() == Some(&b'Z') => {
            let fraction = bytes.get(20..bytes.len() - 1)?;
            if fraction.is_empty() || fraction.len() > 9 {
                return None;
            }
            let mut value = ascii_number(fraction)?;
            for _ in fraction.len()..9 {
                value *= 10;
            }
            value
        }
        _ => return None,
    };

    let calendar = time::Date::from_calendar_date(
        -i32::try_from(year).ok()?,
        Month::try_from(u8::try_from(month).ok()?).ok()?,
        u8::try_from(day).ok()?,
    )
    .ok()?;
    let clock = Time::from_hms_nano(
        u8::try_from(hour).ok()?,
        u8::try_from(minute).ok()?,
        u8::try_from(second).ok()?,
        nanos,
    )
    .ok()?;
    let unix_nanos = PrimitiveDateTime::new(calendar, clock)
        .assume_utc()
        .unix_timestamp_nanos();
    Some(Date::from_unix(
        i64::try_from(unix_nanos.div_euclid(NANOS_PER_SECOND)).ok()?,
        i64::try_from(unix_nanos.rem_euclid(NANOS_PER_SECOND)).ok()?,
    ))
}

fn uid_payload_error() -> Error {
    Error::Message("uid sentinel payload must be an unsigned integer".to_owned())
}

fn date_payload_error() -> Error {
    Error::Message("date sentinel payload must be an rfc 3339 string".to_owned())
}

/// The [`Serializer`] building an owned [`Value`] tree; stateless, a
/// switch over the value kind.
pub(crate) struct ValueSerializer;

macro_rules! serialize_integer {
    ($($method:ident: $ty:ty,)*) => {$(
        fn $method(self, value: $ty) -> Result<Value> {
            Ok(Value::Integer(Integer::from(value)))
        }
    )*};
}

impl Serializer for ValueSerializer {
    type Ok = Value;
    type Error = Error;
    type SerializeSeq = SeqBuilder;
    type SerializeTuple = SeqBuilder;
    type SerializeTupleStruct = SeqBuilder;
    type SerializeTupleVariant = VariantSeqBuilder;
    type SerializeMap = MapBuilder;
    type SerializeStruct = MapBuilder;
    type SerializeStructVariant = VariantMapBuilder;

    serialize_integer! {
        serialize_i8: i8,
        serialize_i16: i16,
        serialize_i32: i32,
        serialize_i64: i64,
        serialize_u8: u8,
        serialize_u16: u16,
        serialize_u32: u32,
        serialize_u64: u64,
    }

    fn serialize_bool(self, value: bool) -> Result<Value> {
        Ok(Value::Boolean(value))
    }

    fn serialize_i128(self, value: i128) -> Result<Value> {
        i64::try_from(value)
            .map(Value::from)
            .or_else(|_| u64::try_from(value).map(Value::from))
            .map_err(|_| Error::UnknownType("i128"))
    }

    fn serialize_u128(self, value: u128) -> Result<Value> {
        u64::try_from(value)
            .map(Value::from)
            .map_err(|_| Error::UnknownType("u128"))
    }

    fn serialize_f32(self, value: f32) -> Result<Value> {
        Ok(Value::Real(Real::from(value)))
    }

    fn serialize_f64(self, value: f64) -> Result<Value> {
        Ok(Value::Real(Real::from(value)))
    }

    fn serialize_char(self, value: char) -> Result<Value> {
        Ok(Value::String(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Value> {
        Ok(Value::String(value.to_owned()))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Value> {
        Ok(Value::Data(value.to_vec()))
    }

    fn serialize_none(self) -> Result<Value> {
        Err(Error::NullNotRepresentable)
    }

    fn serialize_some<T>(self, value: &T) -> Result<Value>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value> {
        Err(Error::NullNotRepresentable)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value> {
        Err(Error::NullNotRepresentable)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Value> {
        Ok(Value::String(variant.to_owned()))
    }

    fn serialize_newtype_struct<T>(self, name: &'static str, value: &T) -> Result<Value>
    where
        T: Serialize + ?Sized,
    {
        match name {
            UID_NEWTYPE => {
                if let Value::Integer(integer) = value.serialize(Self)?
                    && let Some(uid) = integer.as_unsigned()
                {
                    Ok(Value::Uid(Uid::from(uid)))
                } else {
                    Err(uid_payload_error())
                }
            }
            DATE_NEWTYPE => {
                if let Value::String(payload) = value.serialize(Self)?
                    && let Some(date) = parse_sentinel_date(&payload)
                {
                    Ok(Value::Date(date))
                } else {
                    Err(date_payload_error())
                }
            }
            _ => value.serialize(self),
        }
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value>
    where
        T: Serialize + ?Sized,
    {
        let inner = value.serialize(Self)?;
        Ok(Value::Dictionary(Dictionary::from([(
            variant.to_owned(),
            inner,
        )])))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<SeqBuilder> {
        Ok(SeqBuilder::default())
    }

    fn serialize_tuple(self, _len: usize) -> Result<SeqBuilder> {
        Ok(SeqBuilder::default())
    }

    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<SeqBuilder> {
        Ok(SeqBuilder::default())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<VariantSeqBuilder> {
        Ok(VariantSeqBuilder {
            variant,
            elements: SeqBuilder::default(),
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<MapBuilder> {
        Ok(MapBuilder::default())
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<MapBuilder> {
        Ok(MapBuilder::default())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<VariantMapBuilder> {
        Ok(VariantMapBuilder {
            variant,
            entries: MapBuilder::default(),
        })
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

/// Accumulates array elements, dropping the ones that serialize to null
/// (the wire effect of nil holes).
#[derive(Default)]
pub(crate) struct SeqBuilder {
    elements: Vec<Value>,
}

impl SeqBuilder {
    fn push<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        match value.serialize(ValueSerializer) {
            Ok(element) => {
                self.elements.push(element);
                Ok(())
            }
            Err(Error::NullNotRepresentable) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl SerializeSeq for SeqBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Array(self.elements))
    }
}

impl SerializeTuple for SeqBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Array(self.elements))
    }
}

impl SerializeTupleStruct for SeqBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Array(self.elements))
    }
}

/// [`SeqBuilder`] for a tuple variant; `end` wraps the array in the
/// externally-tagged single-key dictionary.
pub(crate) struct VariantSeqBuilder {
    variant: &'static str,
    elements: SeqBuilder,
}

impl SerializeTupleVariant for VariantSeqBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.elements.push(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Dictionary(Dictionary::from([(
            self.variant.to_owned(),
            Value::Array(self.elements.elements),
        )])))
    }
}

/// Accumulates dictionary entries, dropping the ones whose value serializes
/// to null. Duplicate keys overwrite, last writer
/// wins — load-bearing for `#[serde(flatten)]` collisions.
#[derive(Default)]
pub(crate) struct MapBuilder {
    entries: Dictionary,
    pending_key: Option<String>,
}

impl MapBuilder {
    fn insert_pending<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| Error::Message("map value serialized before its key".to_owned()))?;
        match value.serialize(ValueSerializer) {
            Ok(entry) => {
                drop(self.entries.insert(key, entry));
                Ok(())
            }
            Err(Error::NullNotRepresentable) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl SerializeMap for MapBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.pending_key = Some(key.serialize(MapKeySerializer)?);
        Ok(())
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.insert_pending(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Dictionary(self.entries))
    }
}

impl SerializeStruct for MapBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.pending_key = Some(key.to_owned());
        self.insert_pending(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Dictionary(self.entries))
    }
}

/// [`MapBuilder`] for a struct variant; `end` wraps the dictionary in the
/// externally-tagged single-key dictionary.
pub(crate) struct VariantMapBuilder {
    variant: &'static str,
    entries: MapBuilder,
}

impl SerializeStructVariant for VariantMapBuilder {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.entries.pending_key = Some(key.to_owned());
        self.entries.insert_pending(value)
    }

    fn end(self) -> Result<Value> {
        Ok(Value::Dictionary(Dictionary::from([(
            self.variant.to_owned(),
            Value::Dictionary(self.entries.entries),
        )])))
    }
}

/// Accepts only string-kinded map keys (plain strings, newtype strings, and
/// unit variants), rejecting any non-string key.
pub(crate) struct MapKeySerializer;

macro_rules! reject_key {
    ($($method:ident($($arg:ty),*) -> $label:literal,)*) => {$(
        fn $method(self, $(_: $arg),*) -> Result<String> {
            Err(Error::UnknownType($label))
        }
    )*};
}

impl Serializer for MapKeySerializer {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    reject_key! {
        serialize_bool(bool) -> "boolean map key",
        serialize_i8(i8) -> "integer map key",
        serialize_i16(i16) -> "integer map key",
        serialize_i32(i32) -> "integer map key",
        serialize_i64(i64) -> "integer map key",
        serialize_i128(i128) -> "integer map key",
        serialize_u8(u8) -> "integer map key",
        serialize_u16(u16) -> "integer map key",
        serialize_u32(u32) -> "integer map key",
        serialize_u64(u64) -> "integer map key",
        serialize_u128(u128) -> "integer map key",
        serialize_f32(f32) -> "real map key",
        serialize_f64(f64) -> "real map key",
        serialize_char(char) -> "char map key",
        serialize_bytes(&[u8]) -> "data map key",
        serialize_none() -> "optional map key",
        serialize_unit() -> "unit map key",
        serialize_unit_struct(&'static str) -> "unit map key",
    }

    fn serialize_str(self, value: &str) -> Result<String> {
        Ok(value.to_owned())
    }

    fn serialize_some<T>(self, _value: &T) -> Result<String>
    where
        T: Serialize + ?Sized,
    {
        Err(Error::UnknownType("optional map key"))
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<String> {
        Ok(variant.to_owned())
    }

    fn serialize_newtype_struct<T>(self, name: &'static str, value: &T) -> Result<String>
    where
        T: Serialize + ?Sized,
    {
        match name {
            UID_NEWTYPE => Err(Error::UnknownType("UID map key")),
            DATE_NEWTYPE => Err(Error::UnknownType("date map key")),
            _ => value.serialize(self),
        }
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<String>
    where
        T: Serialize + ?Sized,
    {
        Err(Error::UnknownType("enum map key"))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq> {
        Err(Error::UnknownType("array map key"))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple> {
        Err(Error::UnknownType("array map key"))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Err(Error::UnknownType("array map key"))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        Err(Error::UnknownType("enum map key"))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        Err(Error::UnknownType("dictionary map key"))
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        Err(Error::UnknownType("dictionary map key"))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        Err(Error::UnknownType("enum map key"))
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Dictionary(dict) => serializer.collect_map(dict),
            Self::Array(values) => serializer.collect_seq(values),
            Self::String(s) => serializer.serialize_str(s),
            Self::Integer(integer) => integer.serialize(serializer),
            Self::Real(real) => real.serialize(serializer),
            Self::Boolean(b) => serializer.serialize_bool(*b),
            Self::Uid(uid) => uid.serialize(serializer),
            Self::Data(data) => serializer.serialize_bytes(data),
            Self::Date(date) => date.serialize(serializer),
        }
    }
}

impl Serialize for Integer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Self::Signed(value) => serializer.serialize_i64(value),
            Self::Unsigned(value) => serializer.serialize_u64(value),
        }
    }
}

impl Serialize for Real {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.wide() {
            serializer.serialize_f64(self.value())
        } else {
            serializer.serialize_f32(narrow_f32(*self))
        }
    }
}

impl Serialize for Uid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_newtype_struct(UID_NEWTYPE, &self.get())
    }
}

impl Serialize for Date {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_newtype_struct(DATE_NEWTYPE, &format_sentinel_date(*self))
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::panic,
        reason = "test code: unwrap/panic are the assertions"
    )]

    use serde::Serialize;

    use super::*;

    fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn rfc(s: &str) -> Date {
        Date::parse_rfc3339(s).unwrap()
    }

    fn real_of(value: &Value) -> Real {
        match value {
            Value::Real(real) => *real,
            other => panic!("expected a real, got {other:?}"),
        }
    }

    #[test]
    fn scalar_roots_map_to_their_variants() {
        assert_eq!(to_value(&true).unwrap(), Value::Boolean(true));
        assert_eq!(to_value("x").unwrap(), Value::from("x"));
        assert!(matches!(
            to_value(&-1i8).unwrap(),
            Value::Integer(Integer::Signed(-1))
        ));
        assert!(matches!(
            to_value(&u64::MAX).unwrap(),
            Value::Integer(Integer::Unsigned(u64::MAX))
        ));
        assert_eq!(to_value(&1.5f64).unwrap(), Value::from(1.5));
        assert_eq!(to_value(&[1u8, 2]).unwrap().type_name(), "array");
    }

    #[test]
    fn all_integer_widths_widen_to_their_signedness() {
        for value in [
            to_value(&5i8).unwrap(),
            to_value(&5i16).unwrap(),
            to_value(&5i32).unwrap(),
            to_value(&5i64).unwrap(),
        ] {
            assert!(matches!(value, Value::Integer(Integer::Signed(5))));
        }
        for value in [
            to_value(&5u8).unwrap(),
            to_value(&5u16).unwrap(),
            to_value(&5u32).unwrap(),
            to_value(&5u64).unwrap(),
        ] {
            assert!(matches!(value, Value::Integer(Integer::Unsigned(5))));
        }
    }

    #[test]
    fn real_width_follows_the_serialize_call() {
        let narrow = to_value(&1.5f32).unwrap();
        assert!(!real_of(&narrow).wide());
        assert_eq!(narrow, Value::from(1.5));

        let wide = to_value(&1.5f64).unwrap();
        assert!(real_of(&wide).wide());
    }

    #[test]
    fn char_serializes_as_a_one_char_string() {
        assert_eq!(to_value(&'a').unwrap(), Value::from("a"));
        assert_eq!(to_value(&'£').unwrap(), Value::from("£"));
    }

    #[test]
    fn int128_fits_or_errors() {
        assert!(matches!(
            to_value(&5i128).unwrap(),
            Value::Integer(Integer::Signed(5))
        ));
        assert!(matches!(
            to_value(&-5i128).unwrap(),
            Value::Integer(Integer::Signed(-5))
        ));
        assert!(matches!(
            to_value(&i128::from(u64::MAX)).unwrap(),
            Value::Integer(Integer::Unsigned(u64::MAX))
        ));
        assert!(matches!(
            to_value(&7u128).unwrap(),
            Value::Integer(Integer::Unsigned(7))
        ));
        assert!(matches!(
            to_value(&i128::MAX),
            Err(Error::UnknownType("i128"))
        ));
        assert!(matches!(
            to_value(&i128::MIN),
            Err(Error::UnknownType("i128"))
        ));
        assert!(matches!(
            to_value(&u128::MAX),
            Err(Error::UnknownType("u128"))
        ));
    }

    #[test]
    fn null_roots_error_with_no_root_element() {
        #[derive(Serialize)]
        struct Unit;

        assert!(matches!(to_value(&None::<i32>), Err(Error::NoRootElement)));
        assert!(matches!(to_value(&()), Err(Error::NoRootElement)));
        assert!(matches!(to_value(&Unit), Err(Error::NoRootElement)));
    }

    #[test]
    fn containers_drop_null_elements_and_entries() {
        #[derive(Serialize)]
        struct WithOption {
            present: u8,
            absent: Option<u8>,
        }

        let array = to_value(&vec![Some(1u8), None, Some(2u8)]).unwrap();
        assert_eq!(array, Value::from(vec![Value::from(1u8), Value::from(2u8)]));

        let value = to_value(&WithOption {
            present: 1,
            absent: None,
        })
        .unwrap();
        assert_eq!(value, dict([("present", Value::from(1u8))]));

        let mut map = BTreeMap::new();
        assert!(map.insert("gone".to_owned(), None::<u8>).is_none());
        assert!(map.insert("kept".to_owned(), Some(3u8)).is_none());
        assert_eq!(to_value(&map).unwrap(), dict([("kept", Value::from(3u8))]));
    }

    #[test]
    fn null_variant_payloads_cascade_to_the_enclosing_container() {
        #[derive(Serialize)]
        enum Holder {
            Inner(Option<i32>),
        }
        #[derive(Serialize)]
        struct Outer {
            field: Option<Holder>,
        }

        let dropped = to_value(&Outer {
            field: Some(Holder::Inner(None)),
        })
        .unwrap();
        assert_eq!(dropped, dict([]));

        assert!(matches!(
            to_value(&Holder::Inner(None)),
            Err(Error::NoRootElement)
        ));
    }

    #[test]
    fn map_keys_must_be_string_kinded() {
        #[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
        struct NewtypeKey(String);
        #[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
        enum VariantKey {
            A,
        }

        let int_keys = BTreeMap::from([(1i32, "hi")]);
        assert!(matches!(
            to_value(&int_keys),
            Err(Error::UnknownType("integer map key"))
        ));

        let bool_keys = BTreeMap::from([(true, 1u8)]);
        assert!(matches!(
            to_value(&bool_keys),
            Err(Error::UnknownType("boolean map key"))
        ));

        let char_keys = BTreeMap::from([('a', 1u8)]);
        assert!(matches!(
            to_value(&char_keys),
            Err(Error::UnknownType("char map key"))
        ));

        let string_keys = BTreeMap::from([("k".to_owned(), 1u8)]);
        assert_eq!(
            to_value(&string_keys).unwrap(),
            dict([("k", Value::from(1u8))])
        );

        let newtype_keys = BTreeMap::from([(NewtypeKey("n".to_owned()), 1u8)]);
        assert_eq!(
            to_value(&newtype_keys).unwrap(),
            dict([("n", Value::from(1u8))])
        );

        let variant_keys = BTreeMap::from([(VariantKey::A, 1u8)]);
        assert_eq!(
            to_value(&variant_keys).unwrap(),
            dict([("A", Value::from(1u8))])
        );
    }

    #[test]
    fn enums_are_externally_tagged() {
        #[derive(Serialize)]
        enum Repr {
            Unit,
            New(u8),
            Tuple(u8, u8),
            Struct { f: bool },
        }

        assert_eq!(to_value(&Repr::Unit).unwrap(), Value::from("Unit"));
        assert_eq!(
            to_value(&Repr::New(1)).unwrap(),
            dict([("New", Value::from(1u8))])
        );
        assert_eq!(
            to_value(&Repr::Tuple(1, 2)).unwrap(),
            dict([(
                "Tuple",
                Value::from(vec![Value::from(1u8), Value::from(2u8)])
            )])
        );
        assert_eq!(
            to_value(&Repr::Struct { f: true }).unwrap(),
            dict([("Struct", dict([("f", Value::from(true))]))])
        );
    }

    #[test]
    fn byte_vectors_under_derive_are_arrays_not_data() {
        #[derive(Serialize)]
        struct Bytes {
            raw: Vec<u8>,
        }
        let value = to_value(&Bytes { raw: vec![1, 2] }).unwrap();
        assert_eq!(
            value,
            dict([("raw", Value::from(vec![Value::from(1u8), Value::from(2u8)]))])
        );

        // Only serialize_bytes produces Data; Value::Data routes through it.
        assert_eq!(
            to_value(&Value::Data(vec![1, 2])).unwrap(),
            Value::Data(vec![1, 2])
        );
    }

    #[test]
    fn flatten_collisions_resolve_last_writer_wins() {
        #[derive(Serialize)]
        struct Outer {
            a: u8,
            #[serde(flatten)]
            rest: BTreeMap<String, u8>,
        }

        let outer = Outer {
            a: 1,
            rest: BTreeMap::from([("a".to_owned(), 9u8)]),
        };
        assert_eq!(to_value(&outer).unwrap(), dict([("a", Value::from(9u8))]));
    }

    #[test]
    fn uid_and_date_serialize_to_their_variants() {
        assert_eq!(
            to_value(&Uid::from(1024)).unwrap(),
            Value::Uid(Uid::from(1024))
        );
        assert_eq!(
            to_value(&Uid::from(u64::MAX)).unwrap(),
            Value::Uid(Uid::from(u64::MAX))
        );

        let date = rfc("2013-11-27T00:34:00Z");
        assert_eq!(to_value(&date).unwrap(), Value::Date(date));

        let fractional = rfc("2013-11-27T00:34:00.123456789Z");
        assert_eq!(to_value(&fractional).unwrap(), Value::Date(fractional));

        let ancient = Date::from_apple_epoch(-1e300);
        assert_eq!(to_value(&ancient).unwrap(), Value::Date(ancient));
    }

    #[test]
    fn value_trees_round_trip_structurally() {
        let tree = dict([
            (
                "array",
                Value::from(vec![Value::from(-1i64), Value::from(2u8)]),
            ),
            ("bool", Value::from(true)),
            ("data", Value::Data(vec![0xDE, 0xAD])),
            ("date", Value::Date(rfc("2013-11-27T00:34:00.5Z"))),
            ("narrow", Value::Real(Real::from(32.5f32))),
            ("nested", dict([("uid", Value::Uid(Uid::from(7)))])),
            ("string", Value::from("s")),
            ("unsigned", Value::from(u64::MAX)),
            ("wide", Value::from(1.5)),
        ]);
        let round_tripped = to_value(&tree).unwrap();
        assert_eq!(round_tripped, tree);

        let narrow = round_tripped
            .as_dictionary()
            .and_then(|d| d.get("narrow"))
            .unwrap();
        assert!(!real_of(narrow).wide());
        let wide = round_tripped
            .as_dictionary()
            .and_then(|d| d.get("wide"))
            .unwrap();
        assert!(real_of(wide).wide());
    }

    #[test]
    fn sentinel_payload_misuse_errors() {
        let serializer = ValueSerializer;
        assert!(matches!(
            serializer.serialize_newtype_struct(UID_NEWTYPE, "nope"),
            Err(Error::Message(_))
        ));
        assert!(matches!(
            ValueSerializer.serialize_newtype_struct(UID_NEWTYPE, &-1i32),
            Err(Error::Message(_))
        ));
        assert!(matches!(
            ValueSerializer.serialize_newtype_struct(DATE_NEWTYPE, &5u8),
            Err(Error::Message(_))
        ));
        assert!(matches!(
            ValueSerializer.serialize_newtype_struct(DATE_NEWTYPE, "not a date"),
            Err(Error::Message(_))
        ));
    }

    #[test]
    fn sentinel_date_format_is_lossless_rfc3339() {
        assert_eq!(
            format_sentinel_date(rfc("2013-11-27T00:34:00Z")),
            "2013-11-27T00:34:00Z"
        );
        assert_eq!(
            format_sentinel_date(rfc("2013-11-27T00:34:00.5Z")),
            "2013-11-27T00:34:00.5Z"
        );
        assert_eq!(
            format_sentinel_date(rfc("2013-11-27T00:34:00.000000001Z")),
            "2013-11-27T00:34:00.000000001Z"
        );

        for date in [
            rfc("2013-11-27T00:34:00Z"),
            rfc("2013-11-27T00:34:00.123456789Z"),
            rfc("0001-01-01T00:00:00Z"),
            Date::from_apple_epoch(-1e300),
            Date::from_apple_epoch(1e300),
            Date::from_unix(-1, 500_000_000),
        ] {
            assert_eq!(parse_sentinel_date(&format_sentinel_date(date)), Some(date));
        }
    }

    #[test]
    fn sentinel_date_parse_rejects_malformed_negative_years() {
        for payload in [
            "-",
            "-2013-11-27T00:34:00",
            "-2013-11-27T00:34:00z",
            "-13-11-27T00:34:00Z",
            "-2013-13-27T00:34:00Z",
            "-2013-11-27T00:34:00.Z",
            "-2013-11-27T00:34:00.1234567890Z",
            "-2013-11-27T00:34:00Z ",
        ] {
            assert!(parse_sentinel_date(payload).is_none(), "{payload}");
        }
    }
}
