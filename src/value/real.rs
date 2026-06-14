//! The real (floating-point) model carrying the binary codec's width flag.

use std::cmp::Ordering;

/// A property-list real number.
///
/// The value is always stored as an `f64`; a crate-internal flag records
/// whether it originated as a 64-bit float ("wide") or a 32-bit one. The flag
/// drives the binary codec's `0x22`/`0x23` tag choice and object uniquing but
/// never participates in the public API: equality and ordering compare the
/// numeric value only, with IEEE semantics (`NaN` is not equal to itself).
///
/// `From<f32>` builds a narrow real, `From<f64>` a wide one; XML- and
/// text-parsed reals are always wide.
///
/// # Examples
///
/// ```
/// use apple_plist::Real;
///
/// assert_eq!(Real::from(32.0f32), Real::from(32.0f64));
/// assert_ne!(Real::from(f64::NAN), Real::from(f64::NAN));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Real {
    value: f64,
    wide: bool,
}

impl Real {
    /// Returns the numeric value as an `f64`.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Real;
    ///
    /// assert_eq!(Real::from(1.5f64).value(), 1.5);
    /// assert_eq!(Real::from(1.5f32).value(), 1.5);
    /// ```
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    /// Whether this real is conceptually 64-bit. Read by the binary
    /// generator (tag and dedup width) and the serde serializer.
    #[cfg_attr(
        not(any(test, feature = "serde", feature = "binary")),
        expect(
            dead_code,
            reason = "consumed by the serde bridge and the binary codec; dead only when both are compiled out"
        )
    )]
    pub(crate) const fn wide(self) -> bool {
        self.wide
    }
}

impl From<f32> for Real {
    fn from(value: f32) -> Self {
        Self {
            value: f64::from(value),
            wide: false,
        }
    }
}

impl From<f64> for Real {
    fn from(value: f64) -> Self {
        Self { value, wide: true }
    }
}

impl PartialEq for Real {
    // IEEE numeric equality on the value alone is the contract; NaN != NaN.
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl PartialOrd for Real {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::float_cmp, reason = "test code: bit-exact float expectations")]

    use super::*;

    #[test]
    fn width_follows_the_source_type() {
        assert!(!Real::from(32.0f32).wide());
        assert!(Real::from(32.0f64).wide());
    }

    #[test]
    fn narrow_construction_widens_exactly() {
        assert_eq!(Real::from(f32::MAX).value(), f64::from(f32::MAX));
        assert_eq!(Real::from(0.5f32).value(), 0.5);
    }

    #[test]
    fn equality_is_numeric_and_ignores_width() {
        assert_eq!(Real::from(32.0f32), Real::from(32.0f64));
        assert_ne!(Real::from(32.0f64), Real::from(64.0f64));
        assert_eq!(Real::from(-0.0f64), Real::from(0.0f64));
    }

    #[test]
    fn nan_is_not_equal_to_itself() {
        let nan = Real::from(f64::NAN);
        assert_ne!(nan, nan);
        assert_eq!(nan.partial_cmp(&nan), None);
    }

    #[test]
    fn ordering_is_numeric() {
        assert!(Real::from(1.0f32) < Real::from(2.0f64));
        assert_eq!(
            Real::from(2.0f32).partial_cmp(&Real::from(2.0f64)),
            Some(Ordering::Equal)
        );
    }
}
