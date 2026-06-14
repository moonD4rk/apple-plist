//! The dual signed/unsigned 64-bit integer model.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

/// A property-list integer, carrying its signedness.
///
/// Plists must represent both `u64::MAX` and negative values, so the model is
/// the pair of a raw 64-bit payload and a sign flag rather than a single
/// `i64`. Equality, ordering, and hashing are **numeric**: no codec preserves
/// signedness for non-negative values, so `Signed(5)` and `Unsigned(5)` are
/// the same integer (and hash identically).
///
/// # Examples
///
/// ```
/// use apple_plist::Integer;
///
/// assert_eq!(Integer::from(5i64), Integer::from(5u64));
/// assert!(Integer::from(-1i8) < Integer::from(0u8));
/// ```
#[derive(Clone, Copy, Debug)]
pub enum Integer {
    /// A signed value; the only representation negative integers have.
    Signed(i64),
    /// An unsigned value, covering `i64::MAX + 1 ..= u64::MAX` exclusively.
    Unsigned(u64),
}

impl Integer {
    /// Returns the value as an `i64` when it fits.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Integer;
    ///
    /// assert_eq!(
    ///     Integer::Unsigned(i64::MAX as u64).as_signed(),
    ///     Some(i64::MAX)
    /// );
    /// assert_eq!(Integer::Unsigned(i64::MAX as u64 + 1).as_signed(), None);
    /// assert_eq!(Integer::Signed(-1).as_signed(), Some(-1));
    /// ```
    #[must_use]
    pub const fn as_signed(self) -> Option<i64> {
        match self {
            Self::Signed(value) => Some(value),
            Self::Unsigned(value) => {
                if value <= i64::MAX.cast_unsigned() {
                    Some(value.cast_signed())
                } else {
                    None
                }
            }
        }
    }

    /// Returns the value as a `u64` when it is non-negative.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Integer;
    ///
    /// assert_eq!(Integer::Signed(0).as_unsigned(), Some(0));
    /// assert_eq!(Integer::Signed(-1).as_unsigned(), None);
    /// assert_eq!(Integer::Unsigned(u64::MAX).as_unsigned(), Some(u64::MAX));
    /// ```
    #[must_use]
    pub const fn as_unsigned(self) -> Option<u64> {
        match self {
            Self::Unsigned(value) => Some(value),
            Self::Signed(value) => {
                if value >= 0 {
                    Some(value.cast_unsigned())
                } else {
                    None
                }
            }
        }
    }

    /// The structural `(signed, raw bits)` pair — the binary encoder's dedup
    /// key and the `CF$UID` payload, where signedness is ignored.
    #[cfg(any(test, feature = "binary", feature = "xml", feature = "openstep"))]
    pub(crate) const fn to_raw_parts(self) -> (bool, u64) {
        match self {
            Self::Signed(value) => (true, value.cast_unsigned()),
            Self::Unsigned(value) => (false, value),
        }
    }

    fn widen(self) -> i128 {
        match self {
            Self::Signed(value) => i128::from(value),
            Self::Unsigned(value) => i128::from(value),
        }
    }
}

impl PartialEq for Integer {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::Signed(a), Self::Signed(b)) => a == b,
            (Self::Unsigned(a), Self::Unsigned(b)) => a == b,
            (Self::Signed(s), Self::Unsigned(u)) | (Self::Unsigned(u), Self::Signed(s)) => {
                s >= 0 && s.cast_unsigned() == u
            }
        }
    }
}

impl Eq for Integer {}

impl Hash for Integer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // One canonical input per numeric value: non-negative values hash as
        // u64 regardless of variant; negatives exist only as Signed.
        match *self {
            Self::Unsigned(value) => state.write_u64(value),
            Self::Signed(value) if value >= 0 => state.write_u64(value.cast_unsigned()),
            Self::Signed(value) => state.write_i64(value),
        }
    }
}

impl Ord for Integer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.widen().cmp(&other.widen())
    }
}

impl PartialOrd for Integer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Integer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signed(value) => fmt::Display::fmt(value, f),
            Self::Unsigned(value) => fmt::Display::fmt(value, f),
        }
    }
}

macro_rules! impl_from_signed {
    ($($ty:ty),+) => {$(
        impl From<$ty> for Integer {
            fn from(value: $ty) -> Self {
                Self::Signed(i64::from(value))
            }
        }
    )+};
}

macro_rules! impl_from_unsigned {
    ($($ty:ty),+) => {$(
        impl From<$ty> for Integer {
            fn from(value: $ty) -> Self {
                Self::Unsigned(u64::from(value))
            }
        }
    )+};
}

impl_from_signed!(i8, i16, i32, i64);
impl_from_unsigned!(u8, u16, u32, u64);

#[cfg(test)]
mod tests {
    use std::hash::{BuildHasher, RandomState};

    use super::*;

    fn hashes_equal(a: Integer, b: Integer) -> bool {
        let state = RandomState::new();
        state.hash_one(a) == state.hash_one(b)
    }

    #[test]
    fn equality_is_numeric_across_variants() {
        assert_eq!(Integer::Signed(5), Integer::Unsigned(5));
        assert_eq!(Integer::Unsigned(5), Integer::Signed(5));
        assert_eq!(Integer::Signed(0), Integer::Unsigned(0));
        assert_ne!(Integer::Signed(-1), Integer::Unsigned(u64::MAX));
        assert_ne!(
            Integer::Signed(i64::MIN),
            Integer::Unsigned(i64::MIN.unsigned_abs())
        );
        assert_eq!(
            Integer::Signed(i64::MAX),
            Integer::Unsigned(i64::MAX.cast_unsigned())
        );
    }

    #[test]
    fn equal_values_hash_equally() {
        let pairs = [
            (Integer::Signed(5), Integer::Unsigned(5)),
            (Integer::Signed(0), Integer::Unsigned(0)),
            (
                Integer::Signed(i64::MAX),
                Integer::Unsigned(i64::MAX.cast_unsigned()),
            ),
        ];
        for (a, b) in pairs {
            assert_eq!(a, b);
            assert!(hashes_equal(a, b));
        }
        // Benign documented collision: write_i64(v) == write_u64(v as u64).
        assert!(hashes_equal(
            Integer::Signed(-1),
            Integer::Unsigned(u64::MAX)
        ));
        assert_ne!(Integer::Signed(-1), Integer::Unsigned(u64::MAX));
    }

    #[test]
    fn ordering_is_total_numeric_order() {
        let mut values = [
            Integer::Unsigned(u64::MAX),
            Integer::Signed(-1),
            Integer::Unsigned(0),
            Integer::Signed(i64::MIN),
            Integer::Unsigned(i64::MAX.cast_unsigned() + 1),
            Integer::Signed(i64::MAX),
        ];
        values.sort_unstable();
        assert_eq!(
            values,
            [
                Integer::Signed(i64::MIN),
                Integer::Signed(-1),
                Integer::Unsigned(0),
                Integer::Signed(i64::MAX),
                Integer::Unsigned(i64::MAX.cast_unsigned() + 1),
                Integer::Unsigned(u64::MAX),
            ]
        );
        assert!(Integer::Signed(-1) < Integer::Unsigned(0));
        assert!(Integer::Signed(i64::MAX) < Integer::Unsigned(i64::MAX.cast_unsigned() + 1));
        assert_eq!(
            Integer::Signed(7).cmp(&Integer::Unsigned(7)),
            Ordering::Equal
        );
    }

    #[test]
    fn accessor_boundaries_match_the_spec() {
        assert_eq!(
            Integer::Unsigned(i64::MAX.cast_unsigned()).as_signed(),
            Some(i64::MAX)
        );
        assert_eq!(
            Integer::Unsigned(i64::MAX.cast_unsigned() + 1).as_signed(),
            None
        );
        assert_eq!(Integer::Signed(-1).as_unsigned(), None);
        assert_eq!(Integer::Signed(0).as_unsigned(), Some(0));
        assert_eq!(Integer::Signed(i64::MIN).as_signed(), Some(i64::MIN));
    }

    #[test]
    fn raw_parts_expose_signedness_and_bits() {
        assert_eq!(Integer::Signed(-1).to_raw_parts(), (true, u64::MAX));
        assert_eq!(Integer::Signed(5).to_raw_parts(), (true, 5));
        assert_eq!(Integer::Unsigned(5).to_raw_parts(), (false, 5));
        assert_eq!(
            Integer::Signed(i64::MIN).to_raw_parts(),
            (true, 0x8000_0000_0000_0000)
        );
    }

    #[test]
    fn display_prints_canonical_digits() {
        assert_eq!(Integer::Signed(5).to_string(), "5");
        assert_eq!(Integer::Unsigned(5).to_string(), "5");
        assert_eq!(
            Integer::Signed(-9_223_372_036_854_775_808).to_string(),
            "-9223372036854775808"
        );
        assert_eq!(
            Integer::Unsigned(u64::MAX).to_string(),
            "18446744073709551615"
        );
    }

    #[test]
    fn from_impls_cover_all_eight_fixed_width_ints() {
        assert_eq!(Integer::from(-1i8), Integer::Signed(-1));
        assert_eq!(Integer::from(-1i16), Integer::Signed(-1));
        assert_eq!(Integer::from(-1i32), Integer::Signed(-1));
        assert_eq!(Integer::from(-1i64), Integer::Signed(-1));
        assert_eq!(Integer::from(1u8), Integer::Unsigned(1));
        assert_eq!(Integer::from(1u16), Integer::Unsigned(1));
        assert_eq!(Integer::from(1u32), Integer::Unsigned(1));
        assert_eq!(Integer::from(u64::MAX), Integer::Unsigned(u64::MAX));
    }
}
