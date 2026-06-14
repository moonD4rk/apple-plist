//! C-style scalar parsing and formatting shared by the codecs
//! and the lax decode path.
//!
//! Error *conditions* are fixed (a given input always parses or always
//! fails); error *text* follows this crate's style.

use crate::error::{Error, Result};

/// The fixed quiet-NaN bit pattern returned for a `"nan"` literal; the binary
/// NaN golden fixture pins this payload bit.
const NAN_BITS: u64 = 0x7FF8_0000_0000_0001;

fn syntax_error() -> Error {
    Error::ParseScalar("invalid float literal".to_owned())
}

fn range_error() -> Error {
    Error::ParseScalar("value out of range".to_owned())
}

/// Parses an unsigned 64-bit integer: no sign at all (a leading `+` is
/// rejected, unlike Rust), no underscores, no whitespace.
///
/// `base` is 10 or 16; hex digits are case-insensitive.
pub(crate) fn parse_u64(s: &str, base: u32) -> Result<u64> {
    if s.starts_with(['+', '-']) {
        return Err(Error::ParseScalar(
            "invalid digit found in string".to_owned(),
        ));
    }
    u64::from_str_radix(s, base).map_err(|e| Error::ParseScalar(e.to_string()))
}

/// Parses a signed 64-bit integer: one optional leading `+` or `-`, then
/// digits of `base`; out of range errors.
///
/// `base` is 10 or 16. Rust's `i64::from_str_radix` already covers every
/// accept/reject case for these bases.
pub(crate) fn parse_i64(s: &str, base: u32) -> Result<i64> {
    i64::from_str_radix(s, base).map_err(|e| Error::ParseScalar(e.to_string()))
}

/// Parses a boolean: exactly twelve accepted spellings, everything else
/// errors.
#[cfg(any(test, feature = "serde"))]
pub(crate) fn parse_bool(s: &str) -> Result<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(Error::ParseScalar(format!("invalid boolean literal: {s}"))),
    }
}

/// Parses a 64-bit float.
///
/// Divergences from Rust's `f64::from_str` that this helper papers over:
/// signed `inf`/`infinity` but unsigned-only `nan`; overflow of finite syntax
/// is an error while underflow yields `Ok(0.0)`; C-style digit-group
/// underscores and `0x…p±d` hex floats are accepted.
pub(crate) fn parse_f64(s: &str) -> Result<f64> {
    if let Some(value) = parse_special(s) {
        return Ok(value);
    }
    let stripped;
    let body = if s.contains('_') {
        if !underscore_ok(s) {
            return Err(syntax_error());
        }
        stripped = s.replace('_', "");
        stripped.as_str()
    } else {
        s
    };

    let (neg, magnitude) = split_sign(body);
    if let Some(hex_body) = magnitude
        .strip_prefix("0x")
        .or_else(|| magnitude.strip_prefix("0X"))
    {
        if hex_body.is_empty() {
            return Err(syntax_error());
        }
        return parse_hex_f64(hex_body, neg);
    }
    parse_decimal_f64(body)
}

fn split_sign(s: &str) -> (bool, &str) {
    match (s.strip_prefix('-'), s.strip_prefix('+')) {
        (Some(rest), _) => (true, rest),
        (None, Some(rest)) => (false, rest),
        (None, None) => (false, s),
    }
}

/// Special-value floats: optional sign + `inf`/`infinity`, or unsigned `nan`,
/// case-insensitive and consuming the whole string.
fn parse_special(s: &str) -> Option<f64> {
    let (neg, rest) = split_sign(s);
    if rest.eq_ignore_ascii_case("inf") || rest.eq_ignore_ascii_case("infinity") {
        return Some(if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if rest.len() == s.len() && rest.eq_ignore_ascii_case("nan") {
        return Some(f64::from_bits(NAN_BITS));
    }
    None
}

/// Validates digit-group underscores: each underscore must sit between digits
/// (the base prefix counts as a digit).
fn underscore_ok(s: &str) -> bool {
    const BEGIN: u8 = b'^';
    const DIGIT: u8 = b'0';
    const UNDERSCORE: u8 = b'_';
    const OTHER: u8 = b'!';

    let bytes = s.as_bytes();
    let mut saw = BEGIN;
    let mut i = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut hex = false;
    if bytes.get(i) == Some(&b'0')
        && let Some(&c) = bytes.get(i + 1)
    {
        let lower = c | 0x20;
        if lower == b'b' || lower == b'o' || lower == b'x' {
            hex = lower == b'x';
            saw = DIGIT;
            i += 2;
        }
    }
    while let Some(&c) = bytes.get(i) {
        i += 1;
        if c.is_ascii_digit() || (hex && matches!(c | 0x20, b'a'..=b'f')) {
            saw = DIGIT;
            continue;
        }
        if c == b'_' {
            if saw != DIGIT {
                return false;
            }
            saw = UNDERSCORE;
            continue;
        }
        if saw == UNDERSCORE {
            return false;
        }
        saw = OTHER;
    }
    saw != UNDERSCORE
}

/// Validates the decimal float grammar over the whole string, then delegates
/// the (correctly rounded on both sides) conversion to `f64::from_str`.
/// Overflow of finite syntax maps to a range error; underflow stays `Ok(0.0)`.
fn parse_decimal_f64(s: &str) -> Result<f64> {
    let bytes = s.as_bytes();
    let mut i = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut saw_digits = false;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        saw_digits = true;
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            saw_digits = true;
            i += 1;
        }
    }
    if !saw_digits {
        return Err(syntax_error());
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let mut saw_exp_digits = false;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            saw_exp_digits = true;
            i += 1;
        }
        if !saw_exp_digits {
            return Err(syntax_error());
        }
    }
    if i != bytes.len() {
        return Err(syntax_error());
    }
    let value: f64 = s.parse().map_err(|_| syntax_error())?;
    if value.is_infinite() {
        return Err(range_error());
    }
    Ok(value)
}

const fn hex_digit_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Parses the hex-float body after `0x`: hex mantissa digits with an optional
/// dot, then a mandatory binary exponent `p±d`.
fn parse_hex_f64(s: &str, neg: bool) -> Result<f64> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut mantissa: u64 = 0;
    let mut nd: i64 = 0;
    let mut nd_mant: i64 = 0;
    let mut dp: i64 = 0;
    let mut saw_dot = false;
    let mut saw_digits = false;
    let mut trunc = false;
    while let Some(&c) = bytes.get(i) {
        if c == b'.' {
            if saw_dot {
                break;
            }
            saw_dot = true;
            dp = nd;
        } else if let Some(digit) = hex_digit_value(c) {
            saw_digits = true;
            if digit == 0 && nd == 0 {
                dp = dp.saturating_sub(1);
            } else {
                nd = nd.saturating_add(1);
                if nd_mant < 16 {
                    mantissa = mantissa * 16 + u64::from(digit);
                    nd_mant += 1;
                } else if digit != 0 {
                    trunc = true;
                }
            }
        } else {
            break;
        }
        i += 1;
    }
    if !saw_digits {
        return Err(syntax_error());
    }
    if !saw_dot {
        dp = nd;
    }
    dp = dp.saturating_mul(4);
    let mant_bits_read = nd_mant.saturating_mul(4);

    if !matches!(bytes.get(i), Some(b'p' | b'P')) {
        return Err(syntax_error());
    }
    i += 1;
    let mut exp_sign: i64 = 1;
    match bytes.get(i) {
        Some(b'+') => i += 1,
        Some(b'-') => {
            exp_sign = -1;
            i += 1;
        }
        _ => {}
    }
    if !bytes.get(i).is_some_and(u8::is_ascii_digit) {
        return Err(syntax_error());
    }
    let mut e: i64 = 0;
    while let Some(&c) = bytes.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        if e < 10_000 {
            e = e * 10 + i64::from(c - b'0');
        }
        i += 1;
    }
    if i != bytes.len() {
        return Err(syntax_error());
    }
    dp = dp.saturating_add(e.saturating_mul(exp_sign));

    let exp = if mantissa == 0 {
        0
    } else {
        dp.saturating_sub(mant_bits_read)
    };
    atof_hex(mantissa, exp, neg, trunc)
}

/// Assembles an `f64` from a hex mantissa and binary exponent:
/// round-to-nearest-even with a sticky bit for truncated mantissa digits;
/// overflow is a range error, underflow denormalizes to zero without one.
fn atof_hex(mut mantissa: u64, mut exp: i64, neg: bool, trunc: bool) -> Result<f64> {
    const MANT_BITS: u32 = 52;
    const EXP_BITS: u32 = 11;
    const BIAS: i64 = -1023;
    const MAX_EXP: i64 = (1 << EXP_BITS) + BIAS - 2;
    const MIN_EXP: i64 = BIAS + 1;

    exp = exp.saturating_add(i64::from(MANT_BITS));
    while mantissa != 0 && mantissa >> (MANT_BITS + 2) == 0 {
        mantissa <<= 1;
        exp = exp.saturating_sub(1);
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + MANT_BITS + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp = exp.saturating_add(1);
    }
    while mantissa > 1 && exp < MIN_EXP - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp = exp.saturating_add(1);
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp = exp.saturating_add(2);
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + MANT_BITS) {
            mantissa >>= 1;
            exp = exp.saturating_add(1);
        }
    }
    if mantissa >> MANT_BITS == 0 {
        exp = BIAS;
    }
    if exp > MAX_EXP {
        return Err(range_error());
    }
    let mut bits = mantissa & ((1 << MANT_BITS) - 1);
    bits |= ((exp.saturating_sub(BIAS)) & ((1 << EXP_BITS) - 1)).cast_unsigned() << MANT_BITS;
    if neg {
        bits |= 1 << 63;
    }
    Ok(f64::from_bits(bits))
}

/// Formats a 64-bit float in `%g`-style: shortest round-trip digits;
/// scientific notation iff the decimal exponent is below -4 or at least 6,
/// with a signed, two-digit-minimum exponent; specials are
/// `NaN` / `+Inf` / `-Inf`.
#[cfg(any(test, feature = "xml", feature = "openstep"))]
pub(crate) fn format_f64(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf"
        } else {
            "-Inf"
        }
        .to_owned();
    }
    let shortest = format!("{value:e}");
    let (mantissa, exp_digits) = shortest.split_once('e').unwrap_or((shortest.as_str(), "0"));
    let exp: i64 = exp_digits.parse().unwrap_or(0);
    let (neg, mantissa) = mantissa
        .strip_prefix('-')
        .map_or((false, mantissa), |m| (true, m));

    let mut digits = String::with_capacity(mantissa.len());
    for c in mantissa.chars() {
        if c != '.' {
            digits.push(c);
        }
    }

    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if !(-4..6).contains(&exp) {
        let mut chars = digits.chars();
        if let Some(first) = chars.next() {
            out.push(first);
        }
        let rest = chars.as_str();
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let abs = exp.unsigned_abs();
        if abs < 10 {
            out.push('0');
        }
        out.push_str(&abs.to_string());
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 1..exp.unsigned_abs() {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let point = exp + 1;
        let mut written: i64 = 0;
        for c in digits.chars() {
            if written == point {
                out.push('.');
            }
            out.push(c);
            written += 1;
        }
        while written < point {
            out.push('0');
            written += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

    use super::*;

    fn bits(s: &str) -> u64 {
        parse_f64(s).unwrap().to_bits()
    }

    fn is_syntax(s: &str) -> bool {
        matches!(parse_f64(s), Err(Error::ParseScalar(ref m)) if m == "invalid float literal")
    }

    fn is_range(s: &str) -> bool {
        matches!(parse_f64(s), Err(Error::ParseScalar(ref m)) if m == "value out of range")
    }

    #[test]
    fn parse_u64_rejects_any_sign() {
        assert!(parse_u64("+5", 10).is_err());
        assert!(parse_u64("-5", 10).is_err());
        assert!(parse_u64("+0", 16).is_err());
    }

    #[test]
    fn parse_u64_handles_sign_and_base() {
        assert_eq!(parse_u64("5", 10).unwrap(), 5);
        assert_eq!(parse_u64("007", 10).unwrap(), 7);
        assert_eq!(parse_u64("18446744073709551615", 10).unwrap(), u64::MAX);
        assert_eq!(
            parse_u64("deadbeefFACECAFE", 16).unwrap(),
            0xdead_beef_face_cafe
        );
        assert!(parse_u64("18446744073709551616", 10).is_err());
        assert!(parse_u64("", 10).is_err());
        assert!(parse_u64(" 5", 10).is_err());
        assert!(parse_u64("5_0", 10).is_err());
        assert!(parse_u64("0x10", 10).is_err());
    }

    #[test]
    fn parse_i64_handles_sign_and_base() {
        assert_eq!(parse_i64("+5", 10).unwrap(), 5);
        assert_eq!(parse_i64("-9223372036854775808", 10).unwrap(), i64::MIN);
        assert_eq!(parse_i64("-2a", 16).unwrap(), -42);
        assert!(parse_i64("-9223372036854775809", 10).is_err());
        assert!(parse_i64("", 10).is_err());
        assert!(parse_i64("1_0", 10).is_err());
    }

    #[test]
    fn parse_bool_accepts_exactly_the_twelve_tokens() {
        for s in ["1", "t", "T", "TRUE", "true", "True"] {
            assert!(parse_bool(s).unwrap(), "{s}");
        }
        for s in ["0", "f", "F", "FALSE", "false", "False"] {
            assert!(!parse_bool(s).unwrap(), "{s}");
        }
        for s in ["TrUe", "yes", "no", "", "2", " true"] {
            assert!(parse_bool(s).is_err(), "{s}");
        }
    }

    #[test]
    fn parse_f64_specials_follow_sign_rules() {
        assert_eq!(bits("inf"), f64::INFINITY.to_bits());
        assert_eq!(bits("Inf"), f64::INFINITY.to_bits());
        assert_eq!(bits("INF"), f64::INFINITY.to_bits());
        assert_eq!(bits("+inf"), f64::INFINITY.to_bits());
        assert_eq!(bits("-inf"), f64::NEG_INFINITY.to_bits());
        assert_eq!(bits("infinity"), f64::INFINITY.to_bits());
        assert_eq!(bits("+Infinity"), f64::INFINITY.to_bits());
        assert_eq!(bits("-INFINITY"), f64::NEG_INFINITY.to_bits());
        assert_eq!(bits("nan"), 0x7FF8_0000_0000_0001);
        assert_eq!(bits("NaN"), 0x7FF8_0000_0000_0001);
        assert_eq!(bits("nAN"), 0x7FF8_0000_0000_0001);
        assert!(is_syntax("+nan"));
        assert!(is_syntax("-nan"));
        assert!(is_syntax("infi"));
        assert!(is_syntax("infx"));
        assert!(is_syntax("nanx"));
    }

    #[test]
    fn parse_f64_overflow_errors_and_underflow_is_zero() {
        assert!(is_range("1e999"));
        assert!(is_range("-1e999"));
        assert!(is_range("1e310"));
        assert!(is_range("1.7976931348623159e308"));
        assert_eq!(bits("1e-350"), 0);
        assert_eq!(bits("1e-999"), 0);
        assert_eq!(bits("2.4703282292062327e-324"), 0);
        assert_eq!(bits("2.4703282292062328e-324"), 1);
        assert_eq!(bits("1.7976931348623157e308"), 0x7FEF_FFFF_FFFF_FFFF);
        assert_eq!(bits("17976931348623157e292"), 0x7FEF_FFFF_FFFF_FFFF);
    }

    // Golden decimal-parse vectors: reference float parser, round to f64.
    #[test]
    fn parse_f64_decimal_vectors_are_correct() {
        let vectors: &[(&str, u64)] = &[
            ("0", 0x0000_0000_0000_0000),
            ("-0", 0x8000_0000_0000_0000),
            ("5", 0x4014_0000_0000_0000),
            ("-5.5", 0xC016_0000_0000_0000),
            ("1e5", 0x40F8_6A00_0000_0000),
            ("1E5", 0x40F8_6A00_0000_0000),
            ("1.e3", 0x408F_4000_0000_0000),
            (".25", 0x3FD0_0000_0000_0000),
            ("1.", 0x3FF0_0000_0000_0000),
            ("001.250", 0x3FF4_0000_0000_0000),
            ("3.141592653589793", 0x4009_21FB_5444_2D18),
            ("2.718281828459045e0", 0x4005_BF0A_8B14_5769),
            ("1e-310", 0x0000_1268_8B70_E62B),
            ("5e-324", 0x0000_0000_0000_0001),
            ("1e-323", 0x0000_0000_0000_0002),
        ];
        for &(input, want) in vectors {
            assert_eq!(bits(input), want, "{input}");
        }
    }

    #[test]
    fn parse_f64_rejects_syntax_errors() {
        for s in [
            "", ".", "1e", "1e+", "1d", "1.2.3", "0x", "-0x", "0x1p", "0xp2", "0x.p2", "0x1p+",
            "0x1.2", "-0x2a", "1 ", " 1",
        ] {
            assert!(is_syntax(s), "{s}");
        }
    }

    #[test]
    fn parse_f64_underscores_follow_literal_placement() {
        assert_eq!(bits("1_000.5"), 0x408F_4400_0000_0000);
        assert_eq!(bits("1_2.3_4e5_6"), 0x4BC9_29C7_D37D_0D30);
        assert_eq!(bits("1e1_2"), 0x426D_1A94_A200_0000);
        assert_eq!(bits("0x_1p-2"), 0x3FD0_0000_0000_0000);
        for s in [
            "_1", "1__2", "123_", "1_.5", "1._5", "1_e5", "1e_5", "1e5_", "-_1",
        ] {
            assert!(is_syntax(s), "{s}");
        }
    }

    // Golden hex-float parse vectors: reference float parser, round to f64.
    #[test]
    fn parse_f64_hex_floats_are_correct() {
        let vectors: &[(&str, u64)] = &[
            ("0x1p-2", 0x3FD0_0000_0000_0000),
            ("0x1.8p2", 0x4018_0000_0000_0000),
            ("0X1P+3", 0x4020_0000_0000_0000),
            ("0x.8p1", 0x3FF0_0000_0000_0000),
            ("0x1.p0", 0x3FF0_0000_0000_0000),
            ("0x1fffffffffffffp0", 0x433F_FFFF_FFFF_FFFF),
            ("0x1FFFFFFFFFFFFF1p0", 0x437F_FFFF_FFFF_FFFF),
            ("0x1FFFFFFFFFFFFF8p0", 0x4380_0000_0000_0000),
            ("0x1FFFFFFFFFFFFF8000000001p-24", 0x4440_0000_0000_0000),
            ("0x1p-1074", 0x0000_0000_0000_0001),
            ("0x1p-1075", 0x0000_0000_0000_0000),
            ("0x1.8p-1074", 0x0000_0000_0000_0002),
            ("0x0.0000000000000008p-1018", 0x0000_0000_0000_0000),
        ];
        for &(input, want) in vectors {
            assert_eq!(bits(input), want, "{input}");
        }
        assert_eq!(bits("-0x1.8p2"), 0xC018_0000_0000_0000);
        assert!(is_range("0x1p99999"));
        assert!(is_range("-0x1p1024"));
        assert_eq!(bits("0x0p99999"), 0);
    }

    // Golden format vectors: f64 bits to shortest `%g`-style string.
    #[test]
    fn format_f64_shortest_round_trip() {
        let vectors: &[(u64, &str)] = &[
            (0x0000_0000_0000_0000, "0"),
            (0x8000_0000_0000_0000, "-0"),
            (0x3FF0_0000_0000_0000, "1"),
            (0xBFF0_0000_0000_0000, "-1"),
            (0x3FF8_0000_0000_0000, "1.5"),
            (0x4040_0000_0000_0000, "32"),
            (0x4050_0000_0000_0000, "64"),
            (0x4009_21FB_5444_2D18, "3.141592653589793"),
            (0x40F8_6A00_0000_0000, "100000"),
            (0x412E_847E_0000_0000, "999999"),
            (0x412E_8480_0000_0000, "1e+06"),
            (0x4163_12D0_0000_0000, "1e+07"),
            (0x40FE_240C_9FBE_76C9, "123456.789"),
            (0x4132_D687_E3D7_0A3D, "1.23456789e+06"),
            (0x3F1A_36E2_EB1C_432D, "0.0001"),
            (0x3EE4_F8B5_88E3_68F1, "1e-05"),
            (0x3F20_1F31_F46E_D246, "0.000123"),
            (0x3EEF_7510_4D55_1D69, "1.5e-05"),
            (0x0000_0000_0000_0001, "5e-324"),
            (0x7FEF_FFFF_FFFF_FFFF, "1.7976931348623157e+308"),
            (0x47EF_FFFF_E000_0000, "3.4028234663852886e+38"),
            (0x0010_0000_0000_0000, "2.2250738585072014e-308"),
            (0x4415_AF1D_78B5_8C40, "1e+20"),
            (0x444B_1AE4_D6E2_EF50, "1e+21"),
            (0xC19D_6F34_547D_F3B6, "-1.23456789123e+08"),
            (0x4018_0000_0000_0000, "6"),
            (0x3FB9_9999_9999_999A, "0.1"),
            (0x412E_847F_CCCC_CCCD, "999999.9"),
            (0x41B8_4575_7800_0000, "4.0720524e+08"),
            (0x3FE0_0000_0000_0000, "0.5"),
            (0xBFE0_0000_0000_0000, "-0.5"),
            (0x3E4A_8310_BC7A_31BF, "1.23456e-08"),
        ];
        for &(input, want) in vectors {
            assert_eq!(format_f64(f64::from_bits(input)), want, "{input:#018x}");
        }
        assert_eq!(format_f64(f64::NAN), "NaN");
        assert_eq!(format_f64(f64::INFINITY), "+Inf");
        assert_eq!(format_f64(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn parse_and_format_round_trip_shortest_digits() {
        for &bits_in in &[
            0x4009_21FB_5444_2D18_u64,
            0x0000_0000_0000_0001,
            0x7FEF_FFFF_FFFF_FFFF,
        ] {
            let v = f64::from_bits(bits_in);
            assert_eq!(parse_f64(&format_f64(v)).unwrap().to_bits(), bits_in);
        }
    }
}
