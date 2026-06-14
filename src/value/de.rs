//! The serde bridge from [`Value`] trees to Rust values: the
//! deserialization-side type switch.

#[cfg(test)]
use std::collections::BTreeMap;
use std::{fmt, vec};

use serde::de::{
    self, Deserialize, DeserializeOwned, DeserializeSeed, Deserializer, EnumAccess, MapAccess,
    SeqAccess, Unexpected, VariantAccess, Visitor,
};
use serde::forward_to_deserialize_any;

use crate::date::Date;
use crate::depth::MAX_PARSE_DEPTH;
use crate::error::{Error, Result};
use crate::scalar;
use crate::uid::Uid;
use crate::value::ser::{
    DATE_NEWTYPE, UID_NEWTYPE, format_sentinel_date, narrow_f32, parse_sentinel_date,
};
use crate::value::{Dictionary, Integer, Real, Value};

/// Converts an owned [`Value`] tree into any [`DeserializeOwned`] type,
/// always in strict mode (the lax string coercions are reserved for the
/// OpenStep decode path).
///
/// # Errors
///
/// Returns [`Error::TypeMismatch`] when a value's kind cannot satisfy the
/// requested target, [`Error::MaxDepthExceeded`] for trees nested beyond
/// [`MAX_PARSE_DEPTH`] containers, and [`Error::Message`] for the failures
/// serde itself reports (integer range overflow, missing struct fields,
/// unknown enum variants, arity mismatches). Borrowing targets such as
/// `&str` are unreachable through the [`DeserializeOwned`] bound: the tree
/// is owned, so only owned visit methods are ever called.
///
/// # Examples
///
/// ```
/// use apple_plist::Value;
///
/// let answer: i64 = apple_plist::from_value(Value::from(42u8))?;
/// assert_eq!(answer, 42);
/// # Ok::<(), apple_plist::Error>(())
/// ```
pub fn from_value<T>(value: Value) -> Result<T>
where
    T: DeserializeOwned,
{
    T::deserialize(ValueDeserializer::new(value, false))
}

const fn guard_entry(depth: usize) -> Result<()> {
    if depth > MAX_PARSE_DEPTH {
        Err(Error::MaxDepthExceeded)
    } else {
        Ok(())
    }
}

/// Counts one container frame: entering the 129th nested container fails,
/// matching the parsers' shared cap.
const fn guard_descent(depth: usize) -> Result<usize> {
    if depth >= MAX_PARSE_DEPTH {
        Err(Error::MaxDepthExceeded)
    } else {
        Ok(depth + 1)
    }
}

fn unexpected_value(value: &Value) -> Unexpected<'_> {
    match value {
        Value::Dictionary(_) => Unexpected::Map,
        Value::Array(_) => Unexpected::Seq,
        Value::String(s) => Unexpected::Str(s),
        Value::Integer(Integer::Signed(signed)) => Unexpected::Signed(*signed),
        Value::Integer(Integer::Unsigned(unsigned)) => Unexpected::Unsigned(*unsigned),
        Value::Real(real) => Unexpected::Float(real.value()),
        Value::Boolean(b) => Unexpected::Bool(*b),
        Value::Data(data) => Unexpected::Bytes(data),
        Value::Date(_) => Unexpected::Other("date"),
        Value::Uid(_) => Unexpected::Other("UID"),
    }
}

fn visit_array<'de, V>(values: Vec<Value>, depth: usize, lax: bool, visitor: V) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let len = values.len();
    let mut access = SeqAccessor {
        iter: values.into_iter(),
        depth,
        lax,
    };
    let result = visitor.visit_seq(&mut access)?;
    if access.iter.len() == 0 {
        Ok(result)
    } else {
        Err(de::Error::invalid_length(len, &"fewer elements in array"))
    }
}

fn visit_dictionary<'de, V>(
    dict: Dictionary,
    depth: usize,
    lax: bool,
    visitor: V,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let len = dict.len();
    let mut access = MapAccessor {
        iter: dict.into_iter(),
        pending_value: None,
        depth,
        lax,
    };
    let result = visitor.visit_map(&mut access)?;
    if access.iter.len() == 0 && access.pending_value.is_none() {
        Ok(result)
    } else {
        Err(de::Error::invalid_length(
            len,
            &"fewer entries in dictionary",
        ))
    }
}

/// The [`Deserializer`] walking an owned [`Value`] tree: the value-kind
/// type switch.
///
/// `lax` enables the string-to-scalar coercions applied to OpenStep
/// documents only; `depth` carries the container nesting count toward the
/// shared [`MAX_PARSE_DEPTH`] cap.
pub(crate) struct ValueDeserializer {
    value: Value,
    depth: usize,
    lax: bool,
}

impl ValueDeserializer {
    /// Roots a deserializer over `value`; the decode ladder passes
    /// `lax = true` if and only if format detection reported OpenStep.
    pub(crate) const fn new(value: Value, lax: bool) -> Self {
        Self {
            value,
            depth: 0,
            lax,
        }
    }

    const fn at(value: Value, depth: usize, lax: bool) -> Self {
        Self { value, depth, lax }
    }

    fn integer_target<'de, V>(
        self,
        visitor: V,
        expected: &'static str,
        signed: bool,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let found = self.value.type_name();
        match self.value {
            Value::Integer(Integer::Signed(value)) => visitor.visit_i64(value),
            Value::Integer(Integer::Unsigned(value)) => visitor.visit_u64(value),
            Value::Uid(uid) => visitor.visit_u64(uid.get()),
            Value::String(s) if self.lax => {
                if signed {
                    visitor.visit_i64(scalar::parse_i64(&s, 10)?)
                } else {
                    visitor.visit_u64(scalar::parse_u64(&s, 10)?)
                }
            }
            _ => Err(Error::TypeMismatch { expected, found }),
        }
    }

    fn float_target<'de, V>(self, visitor: V, expected: &'static str) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let found = self.value.type_name();
        match self.value {
            Value::Real(real) => visitor.visit_f64(real.value()),
            Value::String(s) if self.lax => visitor.visit_f64(scalar::parse_f64(&s)?),
            _ => Err(Error::TypeMismatch { expected, found }),
        }
    }

    fn string_target<'de, V>(self, visitor: V, expected: &'static str) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let found = self.value.type_name();
        match self.value {
            Value::String(s) => visitor.visit_string(s),
            _ => Err(Error::TypeMismatch { expected, found }),
        }
    }

    /// The UID arm behind the [`Uid`] sentinel.
    fn uid_sentinel<'de, V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let found = self.value.type_name();
        match self.value {
            Value::Uid(uid) => visitor.visit_u64(uid.get()),
            Value::Integer(Integer::Signed(value)) if value < 0 => visitor.visit_i64(value),
            Value::Integer(Integer::Signed(value)) => visitor.visit_u64(value.cast_unsigned()),
            Value::Integer(Integer::Unsigned(value)) => visitor.visit_u64(value),
            Value::String(s) if self.lax => visitor.visit_u64(scalar::parse_u64(&s, 10)?),
            _ => Err(Error::TypeMismatch {
                expected: "UID",
                found,
            }),
        }
    }

    /// The date arm plus the lax text-layout parse behind the [`Date`]
    /// sentinel.
    fn date_sentinel<'de, V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let found = self.value.type_name();
        match self.value {
            Value::Date(date) => visitor.visit_string(format_sentinel_date(date)),
            Value::String(s) if self.lax => Date::parse_text_layout(&s).map_or_else(
                || Err(Error::ParseScalar(format!("invalid date literal: {s}"))),
                |date| visitor.visit_string(format_sentinel_date(date)),
            ),
            _ => Err(Error::TypeMismatch {
                expected: "date",
                found,
            }),
        }
    }
}

macro_rules! deserialize_signed_integer {
    ($($method:ident: $expected:literal,)*) => {$(
        fn $method<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            self.integer_target(visitor, $expected, true)
        }
    )*};
}

macro_rules! deserialize_unsigned_integer {
    ($($method:ident: $expected:literal,)*) => {$(
        fn $method<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            self.integer_target(visitor, $expected, false)
        }
    )*};
}

impl<'de> Deserializer<'de> for ValueDeserializer {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let Self { value, depth, lax } = self;
        match value {
            Value::Dictionary(dict) => visit_dictionary(dict, guard_descent(depth)?, lax, visitor),
            Value::Array(values) => visit_array(values, guard_descent(depth)?, lax, visitor),
            // Lax never applies here: decoding into `any` keeps the string.
            Value::String(s) => visitor.visit_string(s),
            Value::Integer(Integer::Signed(signed)) if signed < 0 => visitor.visit_i64(signed),
            Value::Integer(Integer::Signed(signed)) => visitor.visit_u64(signed.cast_unsigned()),
            Value::Integer(Integer::Unsigned(unsigned)) => visitor.visit_u64(unsigned),
            Value::Real(real) if real.wide() => visitor.visit_f64(real.value()),
            Value::Real(real) => visitor.visit_f32(narrow_f32(real)),
            Value::Boolean(b) => visitor.visit_bool(b),
            Value::Data(data) => visitor.visit_byte_buf(data),
            Value::Date(date) => visitor.visit_newtype_struct(DatePayloadDeserializer { date }),
            Value::Uid(uid) => visitor.visit_newtype_struct(UidPayloadDeserializer { uid }),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let found = self.value.type_name();
        match self.value {
            Value::Boolean(b) => visitor.visit_bool(b),
            Value::String(s) if self.lax => visitor.visit_bool(scalar::parse_bool(&s)?),
            _ => Err(Error::TypeMismatch {
                expected: "bool",
                found,
            }),
        }
    }

    deserialize_signed_integer! {
        deserialize_i8: "i8",
        deserialize_i16: "i16",
        deserialize_i32: "i32",
        deserialize_i64: "i64",
        deserialize_i128: "i128",
    }

    deserialize_unsigned_integer! {
        deserialize_u8: "u8",
        deserialize_u16: "u16",
        deserialize_u32: "u32",
        deserialize_u64: "u64",
        deserialize_u128: "u128",
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.float_target(visitor, "f32")
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.float_target(visitor, "f64")
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.string_target(visitor, "char")
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.string_target(visitor, "string")
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.string_target(visitor, "string")
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let Self { value, depth, lax } = self;
        let found = value.type_name();
        match value {
            Value::Data(data) => visitor.visit_byte_buf(data),
            Value::Array(values) => visit_array(values, guard_descent(depth)?, lax, visitor),
            // A string never coerces to bytes, in strict or lax mode.
            _ => Err(Error::TypeMismatch {
                expected: "bytes",
                found,
            }),
        }
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        visitor.visit_some(self)
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        drop(visitor);
        Err(Error::TypeMismatch {
            expected: "unit",
            found: self.value.type_name(),
        })
    }

    fn deserialize_unit_struct<V>(self, name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        drop(visitor);
        Err(Error::TypeMismatch {
            expected: name,
            found: self.value.type_name(),
        })
    }

    fn deserialize_newtype_struct<V>(self, name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        match name {
            UID_NEWTYPE => self.uid_sentinel(visitor),
            DATE_NEWTYPE => self.date_sentinel(visitor),
            _ => visitor.visit_newtype_struct(self),
        }
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let Self { value, depth, lax } = self;
        let found = value.type_name();
        match value {
            Value::Array(values) => visit_array(values, guard_descent(depth)?, lax, visitor),
            // Data is a leaf in the depth model: its bytes do not descend.
            Value::Data(data) => {
                let bytes = data
                    .into_iter()
                    .map(|byte| Value::Integer(Integer::from(byte)))
                    .collect();
                visit_array(bytes, depth, lax, visitor)
            }
            _ => Err(Error::TypeMismatch {
                expected: "sequence",
                found,
            }),
        }
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let Self { value, depth, lax } = self;
        let found = value.type_name();
        match value {
            Value::Dictionary(dict) => visit_dictionary(dict, guard_descent(depth)?, lax, visitor),
            _ => Err(Error::TypeMismatch {
                expected: "map",
                found,
            }),
        }
    }

    fn deserialize_struct<V>(
        self,
        name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let Self { value, depth, lax } = self;
        let found = value.type_name();
        match value {
            Value::Dictionary(dict) => visit_dictionary(dict, guard_descent(depth)?, lax, visitor),
            _ => Err(Error::TypeMismatch {
                expected: name,
                found,
            }),
        }
    }

    fn deserialize_enum<V>(
        self,
        name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        guard_entry(self.depth)?;
        let Self { value, depth, lax } = self;
        let found = value.type_name();
        match value {
            Value::String(variant) => visitor.visit_enum(EnumDeserializer {
                variant,
                payload: None,
                depth,
                lax,
            }),
            Value::Dictionary(dict) => {
                let depth = guard_descent(depth)?;
                let mut entries = dict.into_iter();
                match (entries.next(), entries.next()) {
                    (Some((variant, payload)), None) => visitor.visit_enum(EnumDeserializer {
                        variant,
                        payload: Some(payload),
                        depth,
                        lax,
                    }),
                    _ => Err(Error::Message(
                        "expected a single-key dictionary for an enum variant".to_owned(),
                    )),
                }
            }
            _ => Err(Error::TypeMismatch {
                expected: name,
                found,
            }),
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.string_target(visitor, "identifier")
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

struct SeqAccessor {
    iter: vec::IntoIter<Value>,
    depth: usize,
    lax: bool,
}

impl<'de> SeqAccess<'de> for SeqAccessor {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        self.iter.next().map_or(Ok(None), |value| {
            seed.deserialize(ValueDeserializer::at(value, self.depth, self.lax))
                .map(Some)
        })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct MapAccessor {
    iter: indexmap::map::IntoIter<String, Value>,
    pending_value: Option<Value>,
    depth: usize,
    lax: bool,
}

impl<'de> MapAccess<'de> for MapAccessor {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        self.iter.next().map_or(Ok(None), |(key, value)| {
            self.pending_value = Some(value);
            // Keys never lax-coerce: dictionary keys are converted, not parsed.
            seed.deserialize(ValueDeserializer::at(Value::String(key), self.depth, false))
                .map(Some)
        })
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        self.pending_value.take().map_or_else(
            || {
                Err(Error::Message(
                    "map value requested before its key".to_owned(),
                ))
            },
            |value| seed.deserialize(ValueDeserializer::at(value, self.depth, self.lax)),
        )
    }

    fn size_hint(&self) -> Option<usize> {
        Some(
            self.iter
                .len()
                .saturating_add(usize::from(self.pending_value.is_some())),
        )
    }
}

struct EnumDeserializer {
    variant: String,
    payload: Option<Value>,
    depth: usize,
    lax: bool,
}

impl<'de> EnumAccess<'de> for EnumDeserializer {
    type Error = Error;
    type Variant = VariantDeserializer;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, VariantDeserializer)>
    where
        V: DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(ValueDeserializer::at(
            Value::String(self.variant),
            self.depth,
            self.lax,
        ))?;
        Ok((
            variant,
            VariantDeserializer {
                payload: self.payload,
                depth: self.depth,
                lax: self.lax,
            },
        ))
    }
}

struct VariantDeserializer {
    payload: Option<Value>,
    depth: usize,
    lax: bool,
}

impl<'de> VariantAccess<'de> for VariantDeserializer {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        self.payload.map_or(Ok(()), |value| {
            Err(de::Error::invalid_type(
                unexpected_value(&value),
                &"unit variant",
            ))
        })
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>,
    {
        self.payload.map_or_else(
            || {
                Err(de::Error::invalid_type(
                    Unexpected::UnitVariant,
                    &"newtype variant",
                ))
            },
            |value| seed.deserialize(ValueDeserializer::at(value, self.depth, self.lax)),
        )
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        match self.payload {
            Some(Value::Array(values)) => {
                visit_array(values, guard_descent(self.depth)?, self.lax, visitor)
            }
            Some(other) => Err(de::Error::invalid_type(
                unexpected_value(&other),
                &"tuple variant",
            )),
            None => Err(de::Error::invalid_type(
                Unexpected::UnitVariant,
                &"tuple variant",
            )),
        }
    }

    fn struct_variant<V>(self, _fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        match self.payload {
            Some(Value::Dictionary(dict)) => {
                visit_dictionary(dict, guard_descent(self.depth)?, self.lax, visitor)
            }
            Some(other) => Err(de::Error::invalid_type(
                unexpected_value(&other),
                &"struct variant",
            )),
            None => Err(de::Error::invalid_type(
                Unexpected::UnitVariant,
                &"struct variant",
            )),
        }
    }
}

/// Replays a [`Date`] as its sentinel payload string inside
/// `visit_newtype_struct`, so `deserialize_any` rejects every target that
/// does not opt into the newtype callback — a date decodes only into a
/// dedicated date type.
struct DatePayloadDeserializer {
    date: Date,
}

impl<'de> Deserializer<'de> for DatePayloadDeserializer {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_string(format_sentinel_date(self.date))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier
        ignored_any
    }
}

/// Replays a [`Uid`] as its `u64` payload inside `visit_newtype_struct`;
/// integer targets take the dedicated downgrade path instead.
struct UidPayloadDeserializer {
    uid: Uid,
}

impl<'de> Deserializer<'de> for UidPayloadDeserializer {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_u64(self.uid.get())
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier
        ignored_any
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any property-list value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Boolean(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Integer(Integer::Signed(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Integer(Integer::Unsigned(value)))
    }

    fn visit_i128<E>(self, value: i128) -> Result<Value, E>
    where
        E: de::Error,
    {
        i64::try_from(value)
            .map(|signed| Value::Integer(Integer::Signed(signed)))
            .or_else(|_| {
                u64::try_from(value).map(|unsigned| Value::Integer(Integer::Unsigned(unsigned)))
            })
            .map_err(|_| E::custom("integer does not fit the 64-bit property-list range"))
    }

    fn visit_u128<E>(self, value: u128) -> Result<Value, E>
    where
        E: de::Error,
    {
        u64::try_from(value)
            .map(|unsigned| Value::Integer(Integer::Unsigned(unsigned)))
            .map_err(|_| E::custom("integer does not fit the 64-bit property-list range"))
    }

    fn visit_f32<E>(self, value: f32) -> Result<Value, E> {
        Ok(Value::Real(Real::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E> {
        Ok(Value::Real(Real::from(value)))
    }

    fn visit_char<E>(self, value: char) -> Result<Value, E> {
        Ok(Value::String(value.to_string()))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_bytes<E>(self, value: &[u8]) -> Result<Value, E> {
        Ok(Value::Data(value.to_vec()))
    }

    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Value, E> {
        Ok(Value::Data(value))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        Value::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(element) = seq.next_element()? {
            values.push(element);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut dict = Dictionary::new();
        while let Some((key, value)) = map.next_entry::<String, Self::Value>()? {
            drop(dict.insert(key, value));
        }
        Ok(Value::Dictionary(dict))
    }

    /// The sentinel probe: only [`Uid`] (`u64` payload) and [`Date`]
    /// (RFC 3339 string payload) arrive through the newtype callback, so the
    /// payload type disambiguates and `Value` round-trips losslessly.
    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::Integer(integer) => integer.as_unsigned().map_or_else(
                || {
                    Err(de::Error::custom(
                        "uid sentinel payload must be an unsigned integer",
                    ))
                },
                |uid| Ok(Value::Uid(Uid::from(uid))),
            ),
            Value::String(payload) => {
                parse_sentinel_date(&payload)
                    .map(Value::Date)
                    .ok_or_else(|| {
                        de::Error::invalid_value(
                            Unexpected::Str(&payload),
                            &"an rfc 3339 date sentinel payload",
                        )
                    })
            }
            other => Err(de::Error::invalid_type(
                unexpected_value(&other),
                &"a UID or date sentinel payload",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct IntegerVisitor;

impl Visitor<'_> for IntegerVisitor {
    type Value = Integer;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a property-list integer")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Integer, E> {
        Ok(Integer::Signed(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Integer, E> {
        Ok(Integer::Unsigned(value))
    }

    fn visit_i128<E>(self, value: i128) -> Result<Integer, E>
    where
        E: de::Error,
    {
        i64::try_from(value)
            .map(Integer::Signed)
            .or_else(|_| u64::try_from(value).map(Integer::Unsigned))
            .map_err(|_| E::custom("integer does not fit the 64-bit property-list range"))
    }

    fn visit_u128<E>(self, value: u128) -> Result<Integer, E>
    where
        E: de::Error,
    {
        u64::try_from(value)
            .map(Integer::Unsigned)
            .map_err(|_| E::custom("integer does not fit the 64-bit property-list range"))
    }
}

impl<'de> Deserialize<'de> for Integer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(IntegerVisitor)
    }
}

struct RealVisitor;

impl Visitor<'_> for RealVisitor {
    type Value = Real;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a property-list real")
    }

    fn visit_f32<E>(self, value: f32) -> Result<Real, E> {
        Ok(Real::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Real, E> {
        Ok(Real::from(value))
    }
}

impl<'de> Deserialize<'de> for Real {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(RealVisitor)
    }
}

struct UidVisitor;

impl<'de> Visitor<'de> for UidVisitor {
    type Value = Uid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an unsigned integer UID")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Uid, E> {
        Ok(Uid::from(value))
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Uid, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_u64(self)
    }
}

impl<'de> Deserialize<'de> for Uid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_newtype_struct(UID_NEWTYPE, UidVisitor)
    }
}

struct DateVisitor;

impl<'de> Visitor<'de> for DateVisitor {
    type Value = Date;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an rfc 3339 date string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Date, E>
    where
        E: de::Error,
    {
        parse_sentinel_date(value).ok_or_else(|| E::invalid_value(Unexpected::Str(value), &self))
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Date, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_newtype_struct(DATE_NEWTYPE, DateVisitor)
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::panic,
        reason = "test code: unwrap/panic are the assertions"
    )]

    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::value::ser::to_value;

    fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn rfc(s: &str) -> Date {
        Date::parse_rfc3339(s).unwrap()
    }

    fn lax<T: DeserializeOwned>(value: Value) -> Result<T> {
        T::deserialize(ValueDeserializer::new(value, true))
    }

    fn nested_arrays(containers: usize) -> Value {
        let mut value = Value::Array(Vec::new());
        for _ in 1..containers {
            value = Value::Array(vec![value]);
        }
        value
    }

    #[test]
    fn derived_struct_round_trips_with_attributes() {
        #[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
        struct Inner {
            count: u32,
        }

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Pet {
            #[serde(rename = "Name")]
            name: String,
            #[serde(skip)]
            mood: u8,
            legs: Option<u32>,
            #[serde(default)]
            tag: String,
            nested: Inner,
        }

        let pet = Pet {
            name: "Fido".to_owned(),
            mood: 0,
            legs: Some(4),
            tag: String::new(),
            nested: Inner { count: 2 },
        };
        let value = to_value(&pet).unwrap();
        assert_eq!(
            value,
            dict([
                ("Name", Value::from("Fido")),
                ("legs", Value::from(4u32)),
                ("nested", dict([("count", Value::from(2u32))])),
                ("tag", Value::from("")),
            ])
        );
        assert_eq!(from_value::<Pet>(value).unwrap(), pet);

        // Option and default fields tolerate absence; mood is skipped entirely.
        let sparse = dict([
            ("Name", Value::from("Rex")),
            ("nested", dict([("count", Value::from(0u32))])),
        ]);
        assert_eq!(
            from_value::<Pet>(sparse).unwrap(),
            Pet {
                name: "Rex".to_owned(),
                mood: 0,
                legs: None,
                tag: String::new(),
                nested: Inner { count: 0 },
            }
        );
    }

    #[test]
    fn missing_keys_error_without_option_or_default() {
        #[derive(Deserialize, Debug)]
        struct Strict {
            #[expect(dead_code, reason = "field exists only to be required")]
            required: i64,
        }
        assert!(from_value::<Strict>(dict([])).is_err());
        assert!(from_value::<Strict>(dict([("required", Value::from(1u8))])).is_ok());
    }

    #[test]
    fn extra_keys_are_ignored_even_when_they_hold_dates() {
        #[derive(Deserialize, PartialEq, Debug)]
        struct Small {
            x: i64,
        }
        let value = dict([
            ("ignored", Value::Date(rfc("2013-11-27T00:34:00Z"))),
            ("uid_too", Value::Uid(Uid::from(3))),
            ("x", Value::from(1u8)),
        ]);
        assert_eq!(from_value::<Small>(value).unwrap(), Small { x: 1 });

        let ignored = de::IgnoredAny::deserialize(ValueDeserializer::new(
            Value::Date(rfc("2013-11-27T00:34:00Z")),
            false,
        ));
        assert!(ignored.is_ok());
    }

    #[test]
    fn flatten_round_trips_and_collisions_keep_the_last_writer() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Outer {
            a: u8,
            #[serde(flatten)]
            rest: BTreeMap<String, u8>,
        }

        let outer = Outer {
            a: 1,
            rest: BTreeMap::from([("b".to_owned(), 2u8)]),
        };
        let value = to_value(&outer).unwrap();
        assert_eq!(
            value,
            dict([("a", Value::from(1u8)), ("b", Value::from(2u8))])
        );
        assert_eq!(from_value::<Outer>(value).unwrap(), outer);

        // Collision: the flattened map's "a" overwrites the field on encode,
        // and the field captures it back on decode.
        let colliding = Outer {
            a: 1,
            rest: BTreeMap::from([("a".to_owned(), 9u8)]),
        };
        let wire = to_value(&colliding).unwrap();
        assert_eq!(wire, dict([("a", Value::from(9u8))]));
        assert_eq!(
            from_value::<Outer>(wire).unwrap(),
            Outer {
                a: 9,
                rest: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn illegal_strict_decodes_error() {
        // Strict decode rejects every cross-kind coercion.
        assert!(from_value::<i64>(Value::from("abc")).is_err());
        assert!(from_value::<i64>(Value::Data(vec![0, 16, 4])).is_err());
        assert!(from_value::<i64>(Value::from(34.1)).is_err());
        assert!(from_value::<i64>(Value::from(true)).is_err());
        assert!(from_value::<i64>(Value::Date(rfc("2010-01-01T00:00:00Z"))).is_err());
        assert!(from_value::<bool>(Value::from(0u8)).is_err());
        assert!(from_value::<bool>(Value::from(vec![Value::from(0u8)])).is_err());
        assert!(from_value::<bool>(dict([("a", Value::from(0u8))])).is_err());
        assert!(
            from_value::<[i32; 1]>(Value::from(vec![
                Value::from(true),
                Value::from(true),
                Value::from(true),
            ]))
            .is_err()
        );
        assert!(from_value::<[u8; 3]>(Value::Data(b"Hello".to_vec())).is_err());
    }

    #[test]
    fn integers_never_coerce_to_floats_but_reals_downcast() {
        assert!(matches!(
            from_value::<f64>(Value::from(5u8)),
            Err(Error::TypeMismatch {
                expected: "f64",
                found: "integer",
            })
        ));
        assert!(from_value::<f32>(Value::from(-5i64)).is_err());

        let downcast = from_value::<f32>(Value::from(34.1)).unwrap();
        assert!((f64::from(downcast) - 34.1).abs() < 1e-4);
        let narrow = from_value::<f32>(Value::Real(Real::from(1.5f32))).unwrap();
        assert!((narrow - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn data_decodes_into_byte_shapes_but_never_strings() {
        assert_eq!(
            from_value::<Vec<u8>>(Value::Data(b"Hello".to_vec())).unwrap(),
            b"Hello".to_vec()
        );
        assert_eq!(
            from_value::<[u8; 5]>(Value::Data(b"Hello".to_vec())).unwrap(),
            *b"Hello"
        );
        assert!(from_value::<[u8; 8]>(Value::Data(b"Hello".to_vec())).is_err());
        assert!(from_value::<String>(Value::Data(b"Hello".to_vec())).is_err());
        assert!(lax::<String>(Value::Data(b"Hello".to_vec())).is_err());

        // Arrays of integers decode element-wise into byte targets.
        let array = Value::from(vec![Value::from(1u8), Value::from(2u8)]);
        assert_eq!(from_value::<Vec<u8>>(array.clone()).unwrap(), vec![1, 2]);
        assert_eq!(from_value::<[u8; 2]>(array).unwrap(), [1, 2]);
        let too_big = Value::from(vec![Value::from(256u16)]);
        assert!(from_value::<Vec<u8>>(too_big).is_err());

        // Strings never become bytes, in either mode.
        assert!(from_value::<Vec<u8>>(Value::from("jkl")).is_err());
        assert!(lax::<Vec<u8>>(Value::from("jkl")).is_err());
    }

    #[test]
    fn date_sources_decode_only_into_dates_and_values() {
        let date = rfc("2013-11-27T00:34:00.5Z");
        let value = Value::Date(date);
        assert_eq!(from_value::<Date>(value.clone()).unwrap(), date);
        assert_eq!(from_value::<Value>(value.clone()).unwrap(), value);

        assert!(from_value::<String>(value.clone()).is_err());
        assert!(from_value::<u64>(value.clone()).is_err());
        assert!(from_value::<f64>(value.clone()).is_err());
        assert!(from_value::<BTreeMap<String, Value>>(value.clone()).is_err());
        assert!(from_value::<Vec<Value>>(value).is_err());

        // A plist string never decodes into Date in strict mode.
        assert!(from_value::<Date>(Value::from("2013-11-27T00:34:00Z")).is_err());

        let ancient = Date::from_apple_epoch(-1e300);
        assert_eq!(
            from_value::<Value>(Value::Date(ancient)).unwrap(),
            Value::Date(ancient)
        );
    }

    #[test]
    fn uid_sources_downgrade_into_integers_but_not_strings_or_floats() {
        let value = Value::Uid(Uid::from(1024));
        assert_eq!(from_value::<Uid>(value.clone()).unwrap(), Uid::from(1024));
        assert_eq!(from_value::<u64>(value.clone()).unwrap(), 1024);
        assert_eq!(from_value::<i64>(value.clone()).unwrap(), 1024);
        assert_eq!(from_value::<Value>(value.clone()).unwrap(), value);
        assert!(from_value::<String>(value.clone()).is_err());
        assert!(from_value::<f64>(value).is_err());

        // Narrow integer targets are range-checked, not wrapped.
        assert!(from_value::<u8>(Value::Uid(Uid::from(1024))).is_err());
        assert!(from_value::<i64>(Value::Uid(Uid::from(u64::MAX))).is_err());
    }

    #[test]
    fn integers_and_lax_strings_decode_into_uid_targets() {
        assert_eq!(
            from_value::<Uid>(Value::from(1024u64)).unwrap(),
            Uid::from(1024)
        );
        assert_eq!(
            from_value::<Uid>(Value::from(1024i64)).unwrap(),
            Uid::from(1024)
        );
        assert!(from_value::<Uid>(Value::from(-1i64)).is_err());
        assert!(from_value::<Uid>(Value::from("12")).is_err());
        assert_eq!(lax::<Uid>(Value::from("12")).unwrap(), Uid::from(12));
        assert!(lax::<Uid>(Value::from("+5")).is_err());
        assert!(from_value::<Uid>(Value::from(1.5)).is_err());
    }

    #[test]
    fn integer_narrowing_is_range_checked() {
        assert!(from_value::<i64>(Value::from(u64::MAX)).is_err());
        assert!(from_value::<i8>(Value::from(300u16)).is_err());
        assert!(from_value::<u64>(Value::from(-1i8)).is_err());
        assert_eq!(from_value::<i64>(Value::from(5u64)).unwrap(), 5);
        assert_eq!(from_value::<u64>(Value::from(5i64)).unwrap(), 5);
        assert_eq!(from_value::<i8>(Value::from(-128i64)).unwrap(), i8::MIN);
    }

    #[test]
    fn int128_targets_fit_or_error() {
        assert_eq!(
            from_value::<i128>(Value::from(u64::MAX)).unwrap(),
            i128::from(u64::MAX)
        );
        assert_eq!(from_value::<u128>(Value::from(7u8)).unwrap(), 7);
        assert!(from_value::<u128>(Value::from(-1i64)).is_err());
        assert_eq!(from_value::<i128>(Value::from(-1i64)).unwrap(), -1);
    }

    #[test]
    fn value_round_trip_is_lossless_for_every_variant() {
        let tree = dict([
            (
                "array",
                Value::from(vec![
                    Value::from(-1i64),
                    Value::from(u64::MAX),
                    dict([("inner", Value::from(false))]),
                ]),
            ),
            ("bool", Value::from(true)),
            ("data", Value::Data(vec![1, 2, 3])),
            ("date", Value::Date(rfc("2013-11-27T00:34:00.123456789Z"))),
            ("narrow", Value::Real(Real::from(32.5f32))),
            ("string", Value::from("hello")),
            ("uid", Value::Uid(Uid::from(u64::MAX))),
            ("wide", Value::from(1.5)),
        ]);
        let round_tripped = from_value::<Value>(tree.clone()).unwrap();
        assert_eq!(round_tripped, tree);

        // The narrow flag survives the generic round trip: a narrow real
        // stays a 32-bit-width real.
        match round_tripped.as_dictionary().and_then(|d| d.get("narrow")) {
            Some(Value::Real(real)) => assert!(!real.wide()),
            other => panic!("expected a real, got {other:?}"),
        }
        match round_tripped.as_dictionary().and_then(|d| d.get("wide")) {
            Some(Value::Real(real)) => assert!(real.wide()),
            other => panic!("expected a real, got {other:?}"),
        }
    }

    #[test]
    fn depth_guard_caps_container_nesting_at_128() {
        assert!(from_value::<Value>(nested_arrays(128)).is_ok());
        assert!(matches!(
            from_value::<Value>(nested_arrays(129)),
            Err(Error::MaxDepthExceeded)
        ));
        // A leaf inside the deepest allowed container still decodes.
        let mut leafy = Value::from(vec![Value::from(1u8)]);
        for _ in 1..128 {
            leafy = Value::from(vec![leafy]);
        }
        assert!(from_value::<Value>(leafy).is_ok());
        // Hostile depth fails fast instead of overflowing the stack.
        assert!(matches!(
            from_value::<Value>(nested_arrays(2000)),
            Err(Error::MaxDepthExceeded)
        ));
    }

    #[test]
    fn lax_decodes_the_lax_fixture() {
        // Lax decode fixture: every scalar arrives as a string from the
        // OpenStep parser.
        #[derive(Deserialize, PartialEq, Debug)]
        #[serde(rename_all = "UPPERCASE")]
        struct LaxTestData {
            i64: i64,
            u64: u64,
            f64: f64,
            b: bool,
            d: Date,
        }

        let value = dict([
            ("B", Value::from("1")),
            ("D", Value::from("2013-11-27 00:34:00 +0000")),
            ("F64", Value::from("3.0")),
            ("I64", Value::from("1")),
            ("U64", Value::from("2")),
        ]);
        assert_eq!(
            lax::<LaxTestData>(value).unwrap(),
            LaxTestData {
                i64: 1,
                u64: 2,
                f64: 3.0,
                b: true,
                d: rfc("2013-11-27T00:34:00Z"),
            }
        );
    }

    #[test]
    fn illegal_lax_decodes_error() {
        // Lax decode still rejects unparseable scalar strings.
        assert!(lax::<i64>(Value::from("abc")).is_err());
        assert!(lax::<u64>(Value::from("abc")).is_err());
        assert!(lax::<f64>(Value::from("def")).is_err());
        assert!(lax::<bool>(Value::from("ghi")).is_err());
        assert!(lax::<Vec<u8>>(Value::from("jkl")).is_err());
    }

    #[test]
    fn lax_bool_accepts_exactly_the_twelve_parse_bool_tokens() {
        for token in ["1", "t", "T", "TRUE", "true", "True"] {
            assert!(lax::<bool>(Value::from(token)).unwrap(), "{token}");
        }
        for token in ["0", "f", "F", "FALSE", "false", "False"] {
            assert!(!lax::<bool>(Value::from(token)).unwrap(), "{token}");
        }
        for token in ["yes", "2", "TrUe", ""] {
            assert!(lax::<bool>(Value::from(token)).is_err(), "{token}");
        }
    }

    #[test]
    fn lax_integers_follow_sign_and_base_rules() {
        assert_eq!(lax::<i64>(Value::from("+5")).unwrap(), 5);
        assert_eq!(lax::<i64>(Value::from("-5")).unwrap(), -5);
        assert!(lax::<u64>(Value::from("+5")).is_err());
        assert!(lax::<u64>(Value::from("-5")).is_err());
        assert_eq!(lax::<u64>(Value::from("5")).unwrap(), 5);
        // Base 10 only: no hex sniffing in lax mode.
        assert!(lax::<i64>(Value::from("0x10")).is_err());
        assert!(lax::<u64>(Value::from("0x10")).is_err());
        assert!(lax::<i64>(Value::from("5_0")).is_err());
        // Parsed at 64 bits, then range-checked into the narrow target.
        assert!(lax::<i8>(Value::from("300")).is_err());
    }

    #[test]
    fn lax_floats_follow_c_style_parse() {
        assert!((lax::<f64>(Value::from("3.0")).unwrap() - 3.0).abs() < f64::EPSILON);
        assert!((lax::<f64>(Value::from("1e3")).unwrap() - 1000.0).abs() < f64::EPSILON);
        assert!(lax::<f64>(Value::from("-Inf")).unwrap().is_infinite());
        assert!(lax::<f64>(Value::from("nan")).unwrap().is_nan());
        assert!(lax::<f64>(Value::from("1e999")).is_err());
        // Via the shared helper: C-style hex floats and digit-group
        // underscores parse.
        assert!((lax::<f64>(Value::from("0x1p-2")).unwrap() - 0.25).abs() < f64::EPSILON);
        assert!((lax::<f64>(Value::from("1_000.5")).unwrap() - 1000.5).abs() < f64::EPSILON);
    }

    #[test]
    fn lax_dates_use_the_text_layout_only() {
        assert_eq!(
            lax::<Date>(Value::from("2013-11-27 00:34:00 +0000")).unwrap(),
            rfc("2013-11-27T00:34:00Z")
        );
        // Offsets convert to UTC.
        assert_eq!(
            lax::<Date>(Value::from("2013-11-27 00:34:00 -0500")).unwrap(),
            rfc("2013-11-27T05:34:00Z")
        );
        assert!(matches!(
            lax::<Date>(Value::from("2013-11-27T00:34:00Z")),
            Err(Error::ParseScalar(_))
        ));
        assert!(lax::<Date>(Value::from("not a date")).is_err());
    }

    #[test]
    fn lax_never_applies_inside_deserialize_any() {
        assert_eq!(lax::<Value>(Value::from("1")).unwrap(), Value::from("1"));
        let map = lax::<BTreeMap<String, Value>>(dict([("B", Value::from("1"))])).unwrap();
        assert_eq!(map.get("B"), Some(&Value::from("1")));
    }

    #[test]
    fn public_from_value_is_always_strict() {
        assert!(from_value::<bool>(Value::from("1")).is_err());
        assert!(from_value::<i64>(Value::from("1")).is_err());
        assert!(from_value::<Date>(Value::from("2013-11-27 00:34:00 +0000")).is_err());
    }

    #[test]
    fn enums_round_trip_their_external_tagging() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        enum Repr {
            Unit,
            New(u8),
            Tuple(u8, u8),
            Struct { f: bool },
        }

        for variant in [
            Repr::Unit,
            Repr::New(1),
            Repr::Tuple(1, 2),
            Repr::Struct { f: true },
        ] {
            let value = to_value(&variant).unwrap();
            assert_eq!(from_value::<Repr>(value).unwrap(), variant);
        }

        assert_eq!(from_value::<Repr>(Value::from("Unit")).unwrap(), Repr::Unit);
        assert_eq!(
            from_value::<Repr>(dict([("New", Value::from(1u8))])).unwrap(),
            Repr::New(1)
        );

        // Shape violations.
        assert!(matches!(
            from_value::<Repr>(dict([
                ("New", Value::from(1u8)),
                ("Unit", Value::from(2u8)),
            ])),
            Err(Error::Message(_))
        ));
        assert!(from_value::<Repr>(dict([])).is_err());
        assert!(from_value::<Repr>(Value::from("New")).is_err());
        assert!(from_value::<Repr>(dict([("Unit", Value::from(1u8))])).is_err());
        assert!(from_value::<Repr>(Value::from("Bogus")).is_err());
        assert!(from_value::<Repr>(Value::from(1u8)).is_err());
    }

    #[test]
    fn map_key_targets_must_be_string_shaped() {
        #[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord, Debug)]
        struct SortaString(String);

        let value = dict([("1", Value::from("first")), ("2", Value::from("second"))]);
        assert!(from_value::<BTreeMap<u32, String>>(value.clone()).is_err());
        // Keys never lax-coerce: map keys are converted, not parsed.
        assert!(lax::<BTreeMap<u32, String>>(value.clone()).is_err());

        let aliased = from_value::<BTreeMap<SortaString, String>>(value).unwrap();
        assert_eq!(
            aliased.get(&SortaString("1".to_owned())),
            Some(&"first".to_owned())
        );
    }

    #[test]
    fn fixed_arity_targets_reject_leftover_elements() {
        let three = Value::from(vec![Value::from(1u8), Value::from(2u8), Value::from(3u8)]);
        assert!(from_value::<(u8, u8)>(three.clone()).is_err());
        assert_eq!(from_value::<(u8, u8, u8)>(three).unwrap(), (1, 2, 3));
        assert!(from_value::<(u8, u8)>(Value::from(vec![Value::from(1u8)])).is_err());
    }

    #[test]
    fn chars_demand_one_character_strings() {
        assert_eq!(from_value::<char>(Value::from("a")).unwrap(), 'a');
        assert_eq!(from_value::<char>(to_value(&'£').unwrap()).unwrap(), '£');
        assert!(from_value::<char>(Value::from("ab")).is_err());
        assert!(from_value::<char>(Value::from("")).is_err());
        assert!(from_value::<char>(Value::from(65u8)).is_err());
    }

    #[test]
    fn borrowing_targets_fail_with_an_owned_tree() {
        let result = <&str>::deserialize(ValueDeserializer::new(Value::from("x"), false));
        assert!(result.is_err());
    }

    #[test]
    fn options_wrap_present_values() {
        assert_eq!(
            from_value::<Option<i64>>(Value::from(5i64)).unwrap(),
            Some(5)
        );
        assert_eq!(
            from_value::<Option<Vec<u8>>>(Value::Data(vec![1])).unwrap(),
            Some(vec![1])
        );
    }

    #[test]
    fn unit_targets_always_mismatch() {
        #[derive(Deserialize, Debug)]
        struct UnitStruct;
        // A braced empty struct accepts any dictionary.
        #[derive(Deserialize, Debug)]
        struct Empty {}

        assert!(from_value::<()>(dict([])).is_err());
        assert!(from_value::<UnitStruct>(dict([])).is_err());
        assert!(from_value::<Empty>(dict([("x", Value::from(1u8))])).is_ok());
        assert!(from_value::<Empty>(Value::from(1u8)).is_err());
    }

    #[test]
    fn special_types_round_trip_inside_derived_structs() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Archive {
            stamp: Date,
            reference: Uid,
        }

        let archive = Archive {
            stamp: rfc("2013-11-27T00:34:00.25Z"),
            reference: Uid::from(42),
        };
        let value = to_value(&archive).unwrap();
        assert_eq!(
            value,
            dict([
                ("reference", Value::Uid(Uid::from(42))),
                ("stamp", Value::Date(rfc("2013-11-27T00:34:00.25Z"))),
            ])
        );
        assert_eq!(from_value::<Archive>(value).unwrap(), archive);
    }

    #[test]
    fn integer_and_real_deserialize_from_their_values() {
        assert_eq!(
            from_value::<Integer>(Value::from(-1i64)).unwrap(),
            Integer::Signed(-1)
        );
        assert_eq!(
            from_value::<Integer>(Value::from(u64::MAX)).unwrap(),
            Integer::Unsigned(u64::MAX)
        );
        assert!(from_value::<Integer>(Value::from(1.5)).is_err());

        let narrow = from_value::<Real>(Value::Real(Real::from(1.5f32))).unwrap();
        assert!(!narrow.wide());
        let wide = from_value::<Real>(Value::from(1.5)).unwrap();
        assert!(wide.wide());
        assert!(from_value::<Real>(Value::from(1u8)).is_err());
    }
}
