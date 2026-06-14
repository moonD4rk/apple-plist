//! Character-class bitmaps for the OpenStep/GNUStep lexer. The bit values are
//! ground truth; never regenerate them from a predicate.

/// A 256-bit membership set over byte values: bit *i* (word `i / 64`, bit
/// `i % 64`) covers byte value *i*. Code points above `0xFF` belong to no
/// set, and neither does end-of-input.
#[derive(Clone, Copy)]
pub(crate) struct CharSet([u64; 4]);

impl CharSet {
    pub(crate) const fn contains_byte(self, byte: u8) -> bool {
        let Self([w0, w1, w2, w3]) = self;
        let word = match byte >> 6 {
            0 => w0,
            1 => w1,
            2 => w2,
            _ => w3,
        };
        (word >> (byte & 63)) & 1 != 0
    }

    pub(crate) fn contains_char(self, ch: char) -> bool {
        u8::try_from(u32::from(ch)).is_ok_and(|byte| self.contains_byte(byte))
    }
}

/// Token-boundary set for unquoted strings in BOTH dialects, and the
/// generator's quoting trigger under GNUStep.
pub(crate) const GS_QUOTABLE: CharSet = CharSet([
    0x7800_1385_ffff_ffff,
    0xa800_0001_3800_0000,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
]);

/// The generator's quoting trigger under OpenStep. Note `;` is absent — a
/// bug-compatible quirk: OpenStep output leaves `;` unquoted even though the
/// parser cannot read it back.
pub(crate) const OS_QUOTABLE: CharSet = CharSet([
    0xf400_7fef_ffff_ffff,
    0xf800_0001_f800_0001,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
]);

/// Inter-token whitespace: BS, TAB, LF, VT, FF, CR, and space. Backspace is
/// bitmap ground truth; do not "fix" it.
pub(crate) const WHITESPACE: CharSet = CharSet([0x0000_0001_0000_3f00, 0, 0, 0]);

/// Line-comment terminators: LF and CR.
pub(crate) const NEWLINE: CharSet = CharSet([0x0000_0000_0000_2400, 0, 0, 0]);

/// Characters kept when filtering a GNUStep `<[...]>` payload before base64
/// decoding: `A-Z a-z 0-9 + / =`.
pub(crate) const BASE64_VALID: CharSet =
    CharSet([0x23ff_8800_0000_0000, 0x07ff_fffe_07ff_fffe, 0, 0]);

#[cfg(test)]
mod tests {
    use super::*;

    fn members(set: CharSet) -> Vec<u8> {
        (0..=255).filter(|&b| set.contains_byte(b)).collect()
    }

    #[test]
    fn whitespace_is_bs_tab_lf_vt_ff_cr_space() {
        assert_eq!(members(WHITESPACE), [8, 9, 10, 11, 12, 13, 32]);
    }

    #[test]
    fn newline_is_lf_and_cr() {
        assert_eq!(members(NEWLINE), [10, 13]);
    }

    #[test]
    fn base64_valid_is_the_standard_alphabet_plus_padding() {
        let mut expected: Vec<u8> = Vec::new();
        expected.extend(b'+'..=b'+');
        expected.extend(b'/'..=b'9');
        expected.push(b'=');
        expected.extend(b'A'..=b'Z');
        expected.extend(b'a'..=b'z');
        assert_eq!(members(BASE64_VALID), expected);
    }

    #[test]
    fn quotable_printable_members_match_the_tables() {
        let printable = |set: CharSet| -> String {
            (0x20..0x7f)
                .filter(|&b| set.contains_byte(b))
                .map(char::from)
                .collect()
        };
        assert_eq!(printable(GS_QUOTABLE), " \"'(),;<=>[\\]`{}");
        assert_eq!(printable(OS_QUOTABLE), " !\"#%&'()*+,-.:<=>?@[\\]^_`{|}~");
    }

    #[test]
    fn quotable_sets_cover_all_controls_and_high_bytes() {
        for set in [GS_QUOTABLE, OS_QUOTABLE] {
            for b in (0x00..0x20).chain(0x7f..=0xff) {
                assert!(set.contains_byte(b), "{b:#04x}");
            }
        }
    }

    #[test]
    fn runes_above_0xff_are_in_no_set() {
        for set in [GS_QUOTABLE, OS_QUOTABLE, WHITESPACE, NEWLINE, BASE64_VALID] {
            assert!(!set.contains_char('\u{100}'));
            assert!(!set.contains_char('世'));
            assert!(!set.contains_char('\u{FFFD}'));
        }
        assert!(GS_QUOTABLE.contains_char('\u{AC}'));
        assert!(GS_QUOTABLE.contains_char(';'));
        assert!(!OS_QUOTABLE.contains_char(';'));
    }
}
