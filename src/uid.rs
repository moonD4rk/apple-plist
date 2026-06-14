//! The `CF$UID` reference type used by Apple keyed archives.

/// A `UID` value, as found in `NSKeyedArchiver` / `CF$UID` property lists.
///
/// In binary plists this is a distinct object kind. In XML and OpenStep it is
/// encoded as the magic single-key dictionary `{ "CF$UID": <integer> }`.
/// Construct one with [`From<u64>`]; read it back with [`Uid::get`] or
/// [`From<Uid>`] for `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uid(u64);

impl Uid {
    /// Returns the underlying integer value.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_plist::Uid;
    ///
    /// assert_eq!(Uid::from(42).get(), 42);
    /// ```
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for Uid {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<Uid> for u64 {
    fn from(uid: Uid) -> Self {
        uid.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_trip() {
        let uid = Uid::from(u64::MAX);
        assert_eq!(uid.get(), u64::MAX);
        assert_eq!(u64::from(uid), u64::MAX);
        assert_eq!(Uid::from(0).get(), 0);
    }

    #[test]
    fn ordering_and_equality_follow_the_value() {
        assert!(Uid::from(1) < Uid::from(2));
        assert_eq!(Uid::from(7), Uid::from(7));
        assert_ne!(Uid::from(7), Uid::from(8));
    }
}
