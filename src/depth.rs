//! The recursion-depth bound shared by every parser.

#[cfg(any(test, feature = "xml"))]
use std::cell::Cell;

/// The maximum structural nesting depth any parser descends before returning
/// [`Error::MaxDepthExceeded`].
///
/// [`Error::MaxDepthExceeded`]: crate::Error::MaxDepthExceeded
pub const MAX_PARSE_DEPTH: usize = 128;

/// RAII depth frame for the XML parser: increment on entry, fail
/// once the new depth exceeds [`MAX_PARSE_DEPTH`], decrement on drop.
///
/// The counter is a [`Cell`] so a live guard does not block the recursive
/// descent that creates the next frame. The binary parser does not use this
/// guard — it bounds its container stack with a `len() >= MAX_PARSE_DEPTH`
/// check before pushing, which accepts a different boundary input set on
/// purpose.
#[cfg(any(test, feature = "xml"))]
pub(crate) struct DepthGuard<'a>(&'a Cell<usize>);

#[cfg(any(test, feature = "xml"))]
impl<'a> DepthGuard<'a> {
    /// Increments `depth`, then fails if the new depth exceeds the cap.
    ///
    /// On failure no guard exists and the counter stays incremented, with no
    /// decrement on the error path; the parse
    /// aborts, so only the success-path balance is observable.
    pub(crate) fn enter(depth: &'a Cell<usize>) -> crate::Result<Self> {
        let entered = depth.get().saturating_add(1);
        depth.set(entered);
        if entered > MAX_PARSE_DEPTH {
            return Err(crate::Error::MaxDepthExceeded);
        }
        Ok(Self(depth))
    }
}

#[cfg(any(test, feature = "xml"))]
impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    fn descend(depth: &Cell<usize>, frames: usize) -> crate::Result<()> {
        let _guard = DepthGuard::enter(depth)?;
        if frames > 1 {
            descend(depth, frames - 1)?;
        }
        Ok(())
    }

    #[test]
    fn cap_is_one_twenty_eight() {
        assert_eq!(MAX_PARSE_DEPTH, 128);
    }

    #[test]
    fn enter_succeeds_up_to_the_cap_and_restores_on_drop() {
        let depth = Cell::new(0);
        assert!(descend(&depth, MAX_PARSE_DEPTH).is_ok());
        assert_eq!(depth.get(), 0);
    }

    #[test]
    fn enter_fails_beyond_the_cap() {
        let depth = Cell::new(0);
        let result = descend(&depth, MAX_PARSE_DEPTH + 1);
        assert!(matches!(result, Err(Error::MaxDepthExceeded)));
    }

    #[test]
    fn early_error_unwinds_only_constructed_guards() {
        let depth = Cell::new(0);
        {
            let _outer = DepthGuard::enter(&depth);
        }
        assert_eq!(depth.get(), 0);

        let depth = Cell::new(MAX_PARSE_DEPTH);
        assert!(matches!(
            DepthGuard::enter(&depth),
            Err(Error::MaxDepthExceeded)
        ));
        // The failed frame is incremented, never decremented.
        assert_eq!(depth.get(), MAX_PARSE_DEPTH + 1);
    }
}
