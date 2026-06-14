//! The owned property-list value tree.

mod integer;
mod real;

#[cfg(feature = "serde")]
pub(crate) mod de;
#[cfg(feature = "serde")]
pub(crate) mod ser;

use indexmap::IndexMap;

pub use self::integer::Integer;
pub use self::real::Real;
use crate::date::Date;
#[cfg(any(test, feature = "xml", feature = "openstep"))]
use crate::scalar;
use crate::uid::Uid;

/// An order-preserving plist dictionary.
///
/// Keys iterate in insertion order, matching how Apple property lists round-trip
/// dictionary keys. Backed by [`IndexMap`], so it carries the full map surface
/// (`get`, `insert`, `entry`, `keys`, `values`, `sort_keys`, indexing).
pub type Dictionary = IndexMap<String, Value>;

/// Any value a property list can hold.
///
/// The tree is owned (`String` / `Vec` / [`Dictionary`], no lifetimes) and
/// covers the nine property-list value kinds. Dictionaries preserve key
/// insertion order.
///
/// The enum is `#[non_exhaustive]`; equality is `PartialEq` only, because a
/// real may hold `NaN`.
///
/// # Examples
///
/// ```
/// use apple_plist::Value;
///
/// let value = Value::from_iter([
///     ("name".to_owned(), Value::from("plist")),
///     ("count".to_owned(), Value::from(3u8)),
/// ]);
/// let dict = value.as_dictionary().expect("built a dictionary");
/// assert_eq!(dict.get("name").and_then(Value::as_str), Some("plist"));
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// A dictionary; keys preserve insertion order.
    Dictionary(Dictionary),
    /// An ordered array.
    Array(Vec<Self>),
    /// A UTF-8 string.
    String(String),
    /// An integer, signed or unsigned.
    Integer(Integer),
    /// A floating-point number.
    Real(Real),
    /// A boolean.
    Boolean(bool),
    /// A keyed-archive `CF$UID` reference.
    Uid(Uid),
    /// Raw bytes.
    Data(Vec<u8>),
    /// An absolute point in time.
    Date(Date),
}

impl Value {
    /// The kind name for this variant, used verbatim in decode-type-mismatch
    /// error messages.
    #[cfg(any(test, feature = "serde"))]
    pub(crate) const fn type_name(&self) -> &'static str {
        match self {
            Self::Dictionary(_) => "dictionary",
            Self::Array(_) => "array",
            Self::String(_) => "string",
            Self::Integer(_) => "integer",
            Self::Real(_) => "real",
            Self::Boolean(_) => "boolean",
            Self::Uid(_) => "UID",
            Self::Data(_) => "data",
            Self::Date(_) => "date",
        }
    }

    /// Returns the dictionary behind [`Value::Dictionary`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Dictionary, Value};
    ///
    /// let value = Value::from(Dictionary::new());
    /// assert!(value.as_dictionary().is_some());
    /// assert!(Value::from(true).as_dictionary().is_none());
    /// ```
    #[must_use]
    pub const fn as_dictionary(&self) -> Option<&Dictionary> {
        match self {
            Self::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Returns the dictionary behind [`Value::Dictionary`], mutably.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Dictionary, Value};
    ///
    /// let mut value = Value::from(Dictionary::new());
    /// if let Some(dict) = value.as_dictionary_mut() {
    ///     let _ = dict.insert("key".to_owned(), Value::from(1u8));
    /// }
    /// assert_eq!(value.as_dictionary().map(Dictionary::len), Some(1));
    /// ```
    #[must_use]
    pub const fn as_dictionary_mut(&mut self) -> Option<&mut Dictionary> {
        match self {
            Self::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Returns the elements behind [`Value::Array`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// let value = Value::from(vec![Value::from(true)]);
    /// assert_eq!(value.as_array().map(Vec::len), Some(1));
    /// assert!(Value::from(true).as_array().is_none());
    /// ```
    #[must_use]
    pub const fn as_array(&self) -> Option<&Vec<Self>> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// Returns the elements behind [`Value::Array`], mutably.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// let mut value = Value::from(Vec::<Value>::new());
    /// if let Some(values) = value.as_array_mut() {
    ///     values.push(Value::from(7i32));
    /// }
    /// assert_eq!(value.as_array().map(Vec::len), Some(1));
    /// ```
    #[must_use]
    pub const fn as_array_mut(&mut self) -> Option<&mut Vec<Self>> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// Returns the string slice behind [`Value::String`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(Value::from("hello").as_str(), Some("hello"));
    /// assert_eq!(Value::from(false).as_str(), None);
    /// ```
    #[must_use]
    pub const fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the integer behind [`Value::Integer`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Integer, Value};
    ///
    /// assert_eq!(Value::from(-1i64).as_integer(), Some(Integer::Signed(-1)));
    /// assert_eq!(Value::from("1").as_integer(), None);
    /// ```
    #[must_use]
    pub const fn as_integer(&self) -> Option<Integer> {
        match self {
            Self::Integer(integer) => Some(*integer),
            _ => None,
        }
    }

    /// Returns the numeric value behind [`Value::Real`] as an `f64`.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(Value::from(1.5).as_real(), Some(1.5));
    /// assert_eq!(Value::from(1u8).as_real(), None);
    /// ```
    #[must_use]
    pub const fn as_real(&self) -> Option<f64> {
        match self {
            Self::Real(real) => Some(real.value()),
            _ => None,
        }
    }

    /// Returns the boolean behind [`Value::Boolean`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(Value::from(true).as_boolean(), Some(true));
    /// assert_eq!(Value::from("true").as_boolean(), None);
    /// ```
    #[must_use]
    pub const fn as_boolean(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the bytes behind [`Value::Data`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(Value::from(vec![1u8, 2]).as_data(), Some(&[1u8, 2][..]));
    /// assert_eq!(Value::from("12").as_data(), None);
    /// ```
    #[must_use]
    pub const fn as_data(&self) -> Option<&[u8]> {
        match self {
            Self::Data(data) => Some(data.as_slice()),
            _ => None,
        }
    }

    /// Returns the date behind [`Value::Date`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use apple_plist::{Date, Value};
    ///
    /// let date = Date::from(SystemTime::UNIX_EPOCH);
    /// assert_eq!(Value::from(date).as_date(), Some(date));
    /// assert_eq!(Value::from(0i64).as_date(), None);
    /// ```
    #[must_use]
    pub const fn as_date(&self) -> Option<Date> {
        match self {
            Self::Date(date) => Some(*date),
            _ => None,
        }
    }

    /// Returns the UID behind [`Value::Uid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Uid, Value};
    ///
    /// assert_eq!(Value::from(Uid::from(7)).as_uid(), Some(Uid::from(7)));
    /// assert_eq!(Value::from(7u64).as_uid(), None);
    /// ```
    #[must_use]
    pub const fn as_uid(&self) -> Option<Uid> {
        match self {
            Self::Uid(uid) => Some(*uid),
            _ => None,
        }
    }

    /// Consumes the value, returning the dictionary behind
    /// [`Value::Dictionary`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::{Dictionary, Value};
    ///
    /// assert_eq!(
    ///     Value::from(Dictionary::new()).into_dictionary(),
    ///     Some(Dictionary::new())
    /// );
    /// assert_eq!(Value::from(true).into_dictionary(), None);
    /// ```
    #[must_use]
    pub fn into_dictionary(self) -> Option<Dictionary> {
        match self {
            Self::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Consumes the value, returning the elements behind [`Value::Array`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(
    ///     Value::from(vec![Value::from(1u8)]).into_array(),
    ///     Some(vec![Value::from(1u8)])
    /// );
    /// assert_eq!(Value::from(1u8).into_array(), None);
    /// ```
    #[must_use]
    pub fn into_array(self) -> Option<Vec<Self>> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// Consumes the value, returning the string behind [`Value::String`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(Value::from("hi").into_string(), Some("hi".to_owned()));
    /// assert_eq!(Value::from(true).into_string(), None);
    /// ```
    #[must_use]
    pub fn into_string(self) -> Option<String> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// Consumes the value, returning the bytes behind [`Value::Data`].
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Value;
    ///
    /// assert_eq!(Value::from(vec![1u8, 2]).into_data(), Some(vec![1u8, 2]));
    /// assert_eq!(Value::from("12").into_data(), None);
    /// ```
    #[must_use]
    pub fn into_data(self) -> Option<Vec<u8>> {
        match self {
            Self::Data(data) => Some(data),
            _ => None,
        }
    }
}

/// Applies the `CF$UID` rewrite to a just-parsed dictionary's raw entry list,
/// before duplicate keys collapse into the map.
///
/// Converts iff there is exactly one pair, its key is byte-exactly `CF$UID`,
/// and its value is an integer (signedness ignored: the raw bits become the
/// UID) — or, when `lax` (OpenStep), a string parsable as a base-10 `u64`.
/// Anything else collapses to a dictionary, last duplicate key winning; there
/// is no error path.
#[cfg(any(test, feature = "xml", feature = "openstep"))]
pub(crate) fn maybe_uid(entries: Vec<(String, Value)>, lax: bool) -> Value {
    if let [(key, value)] = entries.as_slice()
        && key == "CF$UID"
    {
        match value {
            Value::Integer(integer) => return Value::Uid(Uid::from(integer.to_raw_parts().1)),
            Value::String(s) if lax => {
                if let Ok(uid) = scalar::parse_u64(s, 10) {
                    return Value::Uid(Uid::from(uid));
                }
            }
            _ => {}
        }
    }
    Value::Dictionary(entries.into_iter().collect())
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(Real::from(value))
    }
}

impl From<Real> for Value {
    fn from(value: Real) -> Self {
        Self::Real(value)
    }
}

impl From<Integer> for Value {
    fn from(value: Integer) -> Self {
        Self::Integer(value)
    }
}

impl From<Uid> for Value {
    fn from(value: Uid) -> Self {
        Self::Uid(value)
    }
}

impl From<Date> for Value {
    fn from(value: Date) -> Self {
        Self::Date(value)
    }
}

impl From<Vec<Self>> for Value {
    fn from(value: Vec<Self>) -> Self {
        Self::Array(value)
    }
}

impl From<Dictionary> for Value {
    fn from(value: Dictionary) -> Self {
        Self::Dictionary(value)
    }
}

/// The `[]byte → cfData` rule at the literal level: a byte vector is data,
/// never an array. Element-wise arrays go through `FromIterator`.
impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Data(value)
    }
}

macro_rules! impl_from_int {
    ($($ty:ty),+) => {$(
        impl From<$ty> for Value {
            fn from(value: $ty) -> Self {
                Self::Integer(Integer::from(value))
            }
        }
    )+};
}

impl_from_int!(i8, i16, i32, i64, u8, u16, u32, u64);

impl FromIterator<Self> for Value {
    fn from_iter<T: IntoIterator<Item = Self>>(iter: T) -> Self {
        Self::Array(iter.into_iter().collect())
    }
}

/// Builds a dictionary in iteration order; the last value wins for duplicate
/// keys, and the key keeps its first position.
impl FromIterator<(String, Self)> for Value {
    fn from_iter<T: IntoIterator<Item = (String, Self)>>(iter: T) -> Self {
        Self::Dictionary(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, value: Value) -> (String, Value) {
        (key.to_owned(), value)
    }

    #[test]
    fn type_names_are_verbatim() {
        let cases = [
            (Value::Dictionary(Dictionary::new()), "dictionary"),
            (Value::Array(Vec::new()), "array"),
            (Value::from(""), "string"),
            (Value::from(0u8), "integer"),
            (Value::from(0.0), "real"),
            (Value::from(false), "boolean"),
            (Value::from(Uid::from(0)), "UID"),
            (Value::from(Vec::<u8>::new()), "data"),
            (
                Value::from(Date::from(std::time::SystemTime::UNIX_EPOCH)),
                "date",
            ),
        ];
        for (value, want) in cases {
            assert_eq!(value.type_name(), want);
        }
    }

    #[test]
    fn accessors_are_some_only_on_the_matching_variant() {
        let string = Value::from("s");
        assert_eq!(string.as_str(), Some("s"));
        assert!(string.as_integer().is_none());
        assert!(string.as_real().is_none());
        assert!(string.as_boolean().is_none());
        assert!(string.as_data().is_none());
        assert!(string.as_date().is_none());
        assert!(string.as_uid().is_none());
        assert!(string.as_dictionary().is_none());
        assert!(string.as_array().is_none());

        // No coercion: an Integer is not a Real and bytes are not a string.
        assert!(Value::from(5i64).as_real().is_none());
        assert!(Value::from(vec![0x68u8]).as_str().is_none());
    }

    #[test]
    fn mutable_and_consuming_accessors_work() {
        let mut value = Value::from(Dictionary::new());
        if let Some(dict) = value.as_dictionary_mut() {
            drop(dict.insert("k".to_owned(), Value::from(1u8)));
        }
        assert_eq!(value.clone().into_dictionary().map(|d| d.len()), Some(1));
        assert!(value.into_array().is_none());

        let mut array = Value::from(vec![Value::from(1u8)]);
        if let Some(values) = array.as_array_mut() {
            values.push(Value::from(2u8));
        }
        assert_eq!(array.into_array().map(|v| v.len()), Some(2));

        assert_eq!(Value::from("hi").into_string(), Some("hi".to_owned()));
        assert_eq!(Value::from(vec![1u8]).into_data(), Some(vec![1u8]));
        assert!(Value::from(1u8).into_string().is_none());
        assert!(Value::from("x").into_data().is_none());
    }

    #[test]
    fn byte_vectors_become_data_not_arrays() {
        assert_eq!(Value::from(vec![104u8, 105]).type_name(), "data");
        let array: Value = [Value::from(104u8), Value::from(105u8)]
            .into_iter()
            .collect();
        assert_eq!(array.type_name(), "array");
    }

    #[test]
    fn from_f64_is_wide_and_from_real_preserves_width() {
        assert_eq!(Value::from(1.5).as_real(), Some(1.5));
        let narrow = Value::from(Real::from(1.5f32));
        assert_eq!(narrow.as_real(), Some(1.5));
    }

    #[test]
    fn dictionary_from_iterator_keeps_the_last_duplicate_key() {
        let value: Value = [
            entry("dup", Value::from(1u8)),
            entry("other", Value::from(2u8)),
            entry("dup", Value::from(3u8)),
        ]
        .into_iter()
        .collect();
        let dict = value.as_dictionary().cloned().unwrap_or_default();
        assert_eq!(dict.get("dup"), Some(&Value::from(3u8)));
        assert_eq!(dict.len(), 2);
    }

    #[test]
    fn dictionary_iterates_in_insertion_order() {
        let value: Value = [
            entry("zebra", Value::from(1u8)),
            entry("Alpha", Value::from(2u8)),
            entry("alpha", Value::from(3u8)),
        ]
        .into_iter()
        .collect();
        let keys: Vec<&str> = value
            .as_dictionary()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["zebra", "Alpha", "alpha"]);
    }

    #[test]
    fn maybe_uid_collapses_the_single_integer_pair() {
        let converted = maybe_uid(vec![entry("CF$UID", Value::from(255u8))], false);
        assert_eq!(converted, Value::Uid(Uid::from(255)));
    }

    #[test]
    fn maybe_uid_ignores_signedness() {
        let converted = maybe_uid(vec![entry("CF$UID", Value::from(-1i64))], false);
        assert_eq!(converted, Value::Uid(Uid::from(u64::MAX)));
    }

    #[test]
    fn maybe_uid_accepts_numeric_strings_only_when_lax() {
        let lax = maybe_uid(vec![entry("CF$UID", Value::from("12"))], true);
        assert_eq!(lax.as_uid(), Some(Uid::from(12)));

        let strict = maybe_uid(vec![entry("CF$UID", Value::from("12"))], false);
        assert!(strict.as_dictionary().is_some());
    }

    #[test]
    fn maybe_uid_lax_strings_parse_as_u64() {
        for bad in ["+5", "-1", "0x10", "5_0", "", "18446744073709551616", " 5"] {
            let value = maybe_uid(vec![entry("CF$UID", Value::from(bad))], true);
            assert!(value.as_dictionary().is_some(), "{bad}");
        }
        let max = maybe_uid(
            vec![entry("CF$UID", Value::from("18446744073709551615"))],
            true,
        );
        assert_eq!(max.as_uid(), Some(Uid::from(u64::MAX)));
    }

    #[test]
    fn maybe_uid_leaves_other_shapes_as_dictionaries() {
        // Two raw entries — even both named CF$UID — stay a dictionary.
        let duplicate = maybe_uid(
            vec![
                entry("CF$UID", Value::from(1u8)),
                entry("CF$UID", Value::from(2u8)),
            ],
            true,
        );
        let dict = duplicate.as_dictionary().cloned().unwrap_or_default();
        assert_eq!(dict.len(), 1);
        assert_eq!(dict.get("CF$UID"), Some(&Value::from(2u8)));

        for value in [
            maybe_uid(vec![entry("cf$uid", Value::from(1u8))], true),
            maybe_uid(vec![entry("CF$UID ", Value::from(1u8))], true),
            maybe_uid(vec![entry("CF$UID", Value::from(1.0))], true),
            maybe_uid(vec![entry("CF$UID", Value::from(true))], true),
            maybe_uid(Vec::new(), true),
            maybe_uid(
                vec![
                    entry("CF$UID", Value::from(1u8)),
                    entry("extra", Value::from(2u8)),
                ],
                true,
            ),
        ] {
            assert!(value.as_dictionary().is_some());
        }
    }

    #[test]
    fn value_equality_follows_payload_semantics() {
        assert_eq!(Value::from(5i64), Value::from(5u64));
        assert_ne!(Value::from(f64::NAN), Value::from(f64::NAN));
        assert_eq!(Value::from(Real::from(2.0f32)), Value::from(2.0f64));
    }
}
