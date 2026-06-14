//! UTC instants with nanosecond precision, plus the three plist date wire
//! encodings (Apple-epoch seconds, lenient RFC 3339, and the OpenStep text
//! layout), each hand-rolled.

use std::time::{Duration, SystemTime};

#[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
use time::{Month, Time};
use time::{OffsetDateTime, PrimitiveDateTime};

/// The Apple epoch (2001-01-01T00:00:00Z) expressed as Unix seconds.
#[cfg(any(test, feature = "binary"))]
const APPLE_EPOCH_UNIX_SECONDS: f64 = 978_307_200.0;

#[cfg(any(
    test,
    feature = "serde",
    feature = "binary",
    feature = "xml",
    feature = "openstep"
))]
const NANOS_PER_SECOND: i128 = 1_000_000_000;

const MIN_INSTANT: OffsetDateTime = PrimitiveDateTime::MIN.assume_utc();
const MAX_INSTANT: OffsetDateTime = PrimitiveDateTime::MAX.assume_utc();

/// An absolute point in time: a UTC instant with nanosecond precision.
///
/// `Date` exposes no time zone — every constructor normalizes to UTC at its
/// codec boundaries. The representable
/// range spans the years -9999 through 9999; values outside it (reachable
/// only through extreme binary-plist payloads) clamp to the nearest bound.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
///
/// use apple_plist::Date;
///
/// let now = SystemTime::now();
/// let date = Date::from(now);
/// assert_eq!(SystemTime::from(date), now);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(OffsetDateTime);

impl Date {
    /// Builds the instant `secs + nanos / 1e9` relative to the Unix epoch,
    /// normalizing out-of-range nanoseconds (carrying into the seconds field)
    /// and clamping at the representable bounds.
    #[cfg(any(test, feature = "serde", feature = "binary"))]
    pub(crate) fn from_unix(secs: i64, nanos: i64) -> Self {
        Self::clamped(i128::from(secs) * NANOS_PER_SECOND + i128::from(nanos))
    }

    fn clamped(unix_nanos: i128) -> Self {
        OffsetDateTime::from_unix_timestamp_nanos(unix_nanos).map_or_else(
            |_| {
                if unix_nanos < 0 {
                    Self(MIN_INSTANT)
                } else {
                    Self(MAX_INSTANT)
                }
            },
            Self,
        )
    }

    /// Returns `(seconds, subsecond nanoseconds)` since the Unix epoch, with
    /// nanoseconds in `0..1_000_000_000` (the floor convention).
    pub(crate) const fn unix_parts(self) -> (i64, u32) {
        (self.0.unix_timestamp(), self.0.time().nanosecond())
    }

    /// Decodes a binary-plist date payload: seconds since the Apple epoch.
    ///
    /// Never fails: every bit pattern produces a date. Splits the value into
    /// integer and fractional parts (both keep the sign, fractional
    /// nanoseconds truncate toward zero); non-finite and out-of-range values
    /// saturate through the `as` casts and clamp at the representable bounds.
    #[cfg(any(test, feature = "binary"))]
    pub(crate) fn from_apple_epoch(seconds: f64) -> Self {
        let val = seconds + APPLE_EPOCH_UNIX_SECONDS;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "float-to-int conversion site; saturating `as` keeps every payload succeeding (spec 02 §2.7.4)"
        )]
        let (secs, nanos) = (val.trunc() as i64, (val.fract() * 1e9) as i64);
        Self::from_unix(secs, nanos)
    }

    /// Encodes this date as seconds since the Apple epoch.
    ///
    /// Operation order is fixed: one rounding from the combined
    /// integer nanosecond count to `f64`, divide by 1e9, subtract the epoch.
    #[cfg(any(test, feature = "binary"))]
    pub(crate) fn to_apple_epoch(self) -> f64 {
        let (secs, nanos) = self.unix_parts();
        let total = i128::from(secs) * NANOS_PER_SECOND + i128::from(nanos);
        #[expect(
            clippy::cast_precision_loss,
            reason = "single rounding of the combined nanosecond count to f64"
        )]
        let total_f64 = total as f64;
        total_f64 / 1e9 - APPLE_EPOCH_UNIX_SECONDS
    }

    /// Parses an XML plist date with the lenient RFC 3339 grammar: 4-digit
    /// year, 1-or-2-digit hour, optional `.`/`,` fraction,
    /// and a mandatory `Z` or `±hh:mm` zone (offset hour at most 24, minute
    /// at most 60). Returns `None` on any mismatch or range violation.
    #[cfg(any(test, feature = "serde", feature = "xml"))]
    pub(crate) fn parse_rfc3339(input: &str) -> Option<Self> {
        let mut cursor = Cursor::new(input);
        let year = cursor.fixed_digits(4)?;
        cursor.literal(b'-')?;
        let month = cursor.fixed_digits(2)?;
        cursor.literal(b'-')?;
        let day = cursor.fixed_digits(2)?;
        cursor.literal(b'T')?;
        let (hour, minute, second, nanos) = cursor.clock()?;
        let offset_seconds = cursor.rfc3339_offset()?;
        if !cursor.done() {
            return None;
        }
        Self::from_civil(
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanos,
            offset_seconds,
        )
    }

    /// Formats this date for the XML codec: RFC 3339 in UTC, always the `Z`
    /// suffix, sub-second precision silently dropped. Years outside 0..=9999
    /// widen or take a leading `-`.
    #[cfg(any(test, feature = "serde", feature = "xml"))]
    pub(crate) fn format_rfc3339(self) -> String {
        let (year, month, day, hour, minute, second) = self.civil_parts();
        let year = format_year(year);
        format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }

    /// Parses a text-plist date in the `YYYY-MM-DD HH:MM:SS ±hhmm` layout:
    /// 1-or-2-digit hour, optional fraction after the seconds, and a mandatory
    /// `±hhmm` zone (no colon; hour at most 24, minute at most 60).
    #[cfg(any(test, feature = "serde", feature = "openstep"))]
    pub(crate) fn parse_text_layout(input: &str) -> Option<Self> {
        let mut cursor = Cursor::new(input);
        let year = cursor.fixed_digits(4)?;
        cursor.literal(b'-')?;
        let month = cursor.fixed_digits(2)?;
        cursor.literal(b'-')?;
        let day = cursor.fixed_digits(2)?;
        cursor.literal(b' ')?;
        let (hour, minute, second, nanos) = cursor.clock()?;
        cursor.literal(b' ')?;
        let offset_seconds = cursor.numeric_offset()?;
        if !cursor.done() {
            return None;
        }
        Self::from_civil(
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanos,
            offset_seconds,
        )
    }

    /// Formats this date for the text codec: the `YYYY-MM-DD HH:MM:SS ±hhmm`
    /// layout in UTC, so the zone is always `+0000` and sub-second precision
    /// is silently dropped.
    #[cfg(any(test, feature = "openstep"))]
    pub(crate) fn format_text_layout(self) -> String {
        let (year, month, day, hour, minute, second) = self.civil_parts();
        let year = format_year(year);
        format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} +0000")
    }

    #[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
    fn from_civil(
        year: i64,
        month: i64,
        day: i64,
        hour: i64,
        minute: i64,
        second: i64,
        nanos: u32,
        offset_seconds: i64,
    ) -> Option<Self> {
        let month = Month::try_from(u8::try_from(month).ok()?).ok()?;
        let date = time::Date::from_calendar_date(
            i32::try_from(year).ok()?,
            month,
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
        let civil = PrimitiveDateTime::new(date, clock).assume_utc();
        let unix_nanos =
            civil.unix_timestamp_nanos() - i128::from(offset_seconds) * NANOS_PER_SECOND;
        Some(Self::clamped(unix_nanos))
    }

    #[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
    fn civil_parts(self) -> (i32, u8, u8, u8, u8, u8) {
        let date = self.0.date();
        let clock = self.0.time();
        (
            date.year(),
            u8::from(date.month()),
            date.day(),
            clock.hour(),
            clock.minute(),
            clock.second(),
        )
    }
}

impl From<SystemTime> for Date {
    fn from(value: SystemTime) -> Self {
        match value.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(after) => Self::clamped(i128::try_from(after.as_nanos()).unwrap_or(i128::MAX)),
            Err(before) => {
                let nanos = i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX);
                Self::clamped(-nanos)
            }
        }
    }
}

impl From<Date> for SystemTime {
    fn from(value: Date) -> Self {
        let (secs, nanos) = value.unix_parts();
        let result = if secs >= 0 {
            Self::UNIX_EPOCH.checked_add(Duration::new(secs.cast_unsigned(), nanos))
        } else if nanos == 0 {
            Self::UNIX_EPOCH.checked_sub(Duration::new(secs.unsigned_abs(), 0))
        } else {
            let back = Duration::new(secs.unsigned_abs() - 1, 1_000_000_000 - nanos);
            Self::UNIX_EPOCH.checked_sub(back)
        };
        // Unreachable on platforms whose SystemTime spans years ±9999.
        result.unwrap_or(Self::UNIX_EPOCH)
    }
}

#[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
fn format_year(year: i32) -> String {
    if year < 0 {
        format!("-{:04}", year.unsigned_abs())
    } else {
        format!("{year:04}")
    }
}

#[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

#[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
impl<'a> Cursor<'a> {
    const fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn literal(&mut self, expected: u8) -> Option<()> {
        if self.bytes.get(self.pos) == Some(&expected) {
            self.pos += 1;
            Some(())
        } else {
            None
        }
    }

    fn digit(&mut self) -> Option<i64> {
        let &c = self.bytes.get(self.pos)?;
        if c.is_ascii_digit() {
            self.pos += 1;
            Some(i64::from(c - b'0'))
        } else {
            None
        }
    }

    fn fixed_digits(&mut self, count: u32) -> Option<i64> {
        let mut value = 0;
        for _ in 0..count {
            value = value * 10 + self.digit()?;
        }
        Some(value)
    }

    /// Non-fixed numeric field: one digit, or two when a second one follows.
    fn one_or_two_digits(&mut self) -> Option<i64> {
        let first = self.digit()?;
        Some(self.digit().map_or(first, |second| first * 10 + second))
    }

    /// `HH:MM:SS` with a 1-or-2-digit hour and a parse-only fractional
    /// second: `.` or `,` plus at least one digit, kept to 9 digits.
    fn clock(&mut self) -> Option<(i64, i64, i64, u32)> {
        let hour = self.one_or_two_digits()?;
        self.literal(b':')?;
        let minute = self.fixed_digits(2)?;
        self.literal(b':')?;
        let second = self.fixed_digits(2)?;
        Some((hour, minute, second, self.fraction_nanos()))
    }

    fn fraction_nanos(&mut self) -> u32 {
        if !matches!(self.bytes.get(self.pos), Some(b'.' | b',')) {
            return 0;
        }
        if !self.bytes.get(self.pos + 1).is_some_and(u8::is_ascii_digit) {
            return 0;
        }
        self.pos += 1;
        let mut nanos: u32 = 0;
        let mut digits = 0;
        while let Some(&c) = self.bytes.get(self.pos) {
            if !c.is_ascii_digit() {
                break;
            }
            if digits < 9 {
                nanos = nanos * 10 + u32::from(c - b'0');
                digits += 1;
            }
            self.pos += 1;
        }
        while digits < 9 {
            nanos *= 10;
            digits += 1;
        }
        nanos
    }

    /// `Z` or `±hh:mm`, returning the offset in seconds.
    #[cfg(any(test, feature = "serde", feature = "xml"))]
    fn rfc3339_offset(&mut self) -> Option<i64> {
        if self.literal(b'Z').is_some() {
            return Some(0);
        }
        let negative = self.sign()?;
        let hours = self.fixed_digits(2)?;
        self.literal(b':')?;
        let minutes = self.fixed_digits(2)?;
        Self::zone_seconds(hours, minutes, negative)
    }

    /// `±hhmm`, returning the offset in seconds.
    #[cfg(any(test, feature = "serde", feature = "openstep"))]
    fn numeric_offset(&mut self) -> Option<i64> {
        let negative = self.sign()?;
        let hours = self.fixed_digits(2)?;
        let minutes = self.fixed_digits(2)?;
        Self::zone_seconds(hours, minutes, negative)
    }

    fn sign(&mut self) -> Option<bool> {
        match self.bytes.get(self.pos) {
            Some(b'+') => {
                self.pos += 1;
                Some(false)
            }
            Some(b'-') => {
                self.pos += 1;
                Some(true)
            }
            _ => None,
        }
    }

    /// Lenient zone ranges: hour at most 24, minute at most 60.
    const fn zone_seconds(hours: i64, minutes: i64, negative: bool) -> Option<i64> {
        if hours > 24 || minutes > 60 {
            return None;
        }
        let seconds = (hours * 60 + minutes) * 60;
        Some(if negative { -seconds } else { seconds })
    }

    const fn done(&self) -> bool {
        self.pos == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::float_cmp,
        reason = "test code: unwrap is the assertion; float expectations are bit-exact"
    )]

    use super::*;

    fn rfc3339(s: &str) -> Date {
        Date::parse_rfc3339(s).unwrap()
    }

    #[test]
    fn apple_epoch_round_trips_the_golden_fixture() {
        let date = rfc3339("2013-11-27T00:34:00Z");
        let encoded = date.to_apple_epoch();
        assert_eq!(encoded, 407_205_240.0);
        assert_eq!(encoded.to_bits(), 0x41B8_4575_7800_0000);
        assert_eq!(Date::from_apple_epoch(407_205_240.0), date);
    }

    #[test]
    fn apple_epoch_parse_truncates_fractional_nanos_toward_zero() {
        let date = Date::from_apple_epoch(0.5);
        assert_eq!(date.unix_parts(), (978_307_200, 500_000_000));

        // Pre-Unix-epoch: the split yields a negative fraction; the seconds
        // field borrows to normalize the nanoseconds back into range.
        let date = Date::from_apple_epoch(-978_307_200.5);
        assert_eq!(date.unix_parts(), (-1, 500_000_000));
    }

    #[test]
    fn apple_epoch_parse_never_fails_and_clamps() {
        let max = Date::from_apple_epoch(f64::INFINITY);
        assert_eq!(max, Date(MAX_INSTANT));
        assert_eq!(Date::from_apple_epoch(1e300), Date(MAX_INSTANT));
        assert_eq!(Date::from_apple_epoch(f64::NEG_INFINITY), Date(MIN_INSTANT));
        assert_eq!(Date::from_apple_epoch(-1e300), Date(MIN_INSTANT));
        // Rust's saturating cast maps NaN to 0 seconds (the exact value here
        // is implementation-defined; only success parity is contractual).
        assert_eq!(Date::from_apple_epoch(f64::NAN).unix_parts(), (0, 0));
    }

    #[test]
    fn apple_epoch_encode_rounds_through_the_nanosecond_intermediate() {
        // `float64(unix_nanos)/1e9 - epoch` rounds the 61-bit nano count
        // once; the pinned bits below are the expected result.
        let date = Date::from_unix(1_385_512_440, 250_000_000);
        assert_eq!(date.to_apple_epoch().to_bits(), 0x41B8_4575_783F_FFFC);
    }

    #[test]
    fn rfc3339_parse_accepts_the_grammar() {
        assert_eq!(
            rfc3339("2013-11-27T00:34:00Z").unix_parts(),
            (1_385_512_440, 0)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00.5Z").unix_parts(),
            (1_385_512_440, 500_000_000)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00,5Z").unix_parts(),
            (1_385_512_440, 500_000_000)
        );
        assert_eq!(
            rfc3339("2013-11-27T1:34:00Z").unix_parts(),
            (1_385_516_040, 0)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00+07:00").unix_parts(),
            (1_385_487_240, 0)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00-00:30").unix_parts(),
            (1_385_514_240, 0)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00+24:00").unix_parts(),
            (1_385_426_040, 0)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00+23:60").unix_parts(),
            (1_385_426_040, 0)
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00.123456789123Z").unix_parts(),
            (1_385_512_440, 123_456_789)
        );
        assert_eq!(
            rfc3339("0000-01-01T00:00:00+24:00").unix_parts(),
            (-62_167_305_600, 0)
        );
    }

    #[test]
    fn rfc3339_parse_rejects_malformed_input() {
        for s in [
            "",
            "2013-11-27t00:34:00Z",
            "2013-11-27T00:34:00",
            "2013-11-27T00:34:00z",
            "2013-11-27T00:34:00+0700",
            "2013-11-27T00:34:00+25:00",
            "2013-02-30T00:34:00Z",
            "12013-11-27T00:34:00Z",
            "2013-11-27T00:34:60Z",
            "2013-11-27T24:34:00Z",
            "2013-1-27T00:34:00Z",
            "2013-11-27T0:4:00Z",
            "2013-11-27T00:34:00.Z",
            "2013-11-27T00:34:00Z ",
            "2013-13-01T00:00:00Z",
            "2013-00-01T00:00:00Z",
            "2013-11-00T00:00:00Z",
        ] {
            assert!(Date::parse_rfc3339(s).is_none(), "{s}");
        }
    }

    #[test]
    fn rfc3339_parse_clamps_past_the_calendar_edge() {
        // The offset rolls this into year 10000; the time crate tops out at
        // 9999, so the result clamps to the maximum instant.
        let date = rfc3339("9999-12-31T23:59:59-24:00");
        assert_eq!(date, Date(MAX_INSTANT));
    }

    #[test]
    fn rfc3339_format_is_utc_z_with_subseconds_dropped() {
        assert_eq!(
            rfc3339("2013-11-27T00:34:00Z").format_rfc3339(),
            "2013-11-27T00:34:00Z"
        );
        assert_eq!(
            rfc3339("2013-11-27T00:34:00.75Z").format_rfc3339(),
            "2013-11-27T00:34:00Z"
        );
        assert_eq!(
            rfc3339("2013-11-27T05:34:00+05:00").format_rfc3339(),
            "2013-11-27T00:34:00Z"
        );
        assert_eq!(Date(MIN_INSTANT).format_rfc3339(), "-9999-01-01T00:00:00Z");
        assert_eq!(
            rfc3339("0001-02-03T04:05:06Z").format_rfc3339(),
            "0001-02-03T04:05:06Z"
        );
    }

    #[test]
    fn text_layout_parse_accepts_the_grammar() {
        let parse = Date::parse_text_layout;
        assert_eq!(
            parse("2013-11-27 00:34:00 +0000").unwrap().unix_parts(),
            (1_385_512_440, 0)
        );
        assert_eq!(
            parse("2013-11-27 0:34:00 +0000").unwrap().unix_parts(),
            (1_385_512_440, 0)
        );
        assert_eq!(
            parse("2013-11-27 00:34:00.25 +0000").unwrap().unix_parts(),
            (1_385_512_440, 250_000_000)
        );
        assert_eq!(
            parse("2013-11-27 00:34:00,5 +0000").unwrap().unix_parts(),
            (1_385_512_440, 500_000_000)
        );
        assert_eq!(
            parse("2013-11-27 00:34:00 -0500").unwrap().unix_parts(),
            (1_385_530_440, 0)
        );
        assert_eq!(
            parse("2013-11-27 00:34:00 +0060").unwrap().unix_parts(),
            (1_385_508_840, 0)
        );
    }

    #[test]
    fn text_layout_parse_rejects_malformed_input() {
        for s in [
            "",
            "2013-11-27 00:34:00 Z",
            "2013-11-27 00:34:00 +00:00",
            "2013-11-27 00:34:00",
            "2013-11-27 00:34:00 +000",
            "2013-11-27 00:34:00 +9900",
            "2013-11-27 00:34:00 +2500",
            "2013-11-27 00:34:00 +0061",
            "2013-02-30 00:34:00 +0000",
            "2013-11-27T00:34:00 +0000",
            "2013-11-27 00:34:00 +0000 ",
        ] {
            assert!(Date::parse_text_layout(s).is_none(), "{s}");
        }
    }

    #[test]
    fn text_layout_format_is_utc_plus_zero_zero() {
        let date = rfc3339("2013-11-27T00:34:00.9Z");
        assert_eq!(date.format_text_layout(), "2013-11-27 00:34:00 +0000");
    }

    #[test]
    fn text_and_rfc3339_round_trip_whole_second_dates() {
        let date = Date::parse_text_layout("2013-11-27 05:34:00 -0500").unwrap();
        assert_eq!(date.format_text_layout(), "2013-11-27 10:34:00 +0000");
        assert_eq!(Date::parse_rfc3339(&date.format_rfc3339()).unwrap(), date);
        assert_eq!(
            Date::parse_text_layout(&date.format_text_layout()).unwrap(),
            date
        );
    }

    #[test]
    fn system_time_conversions_round_trip() {
        let after = SystemTime::UNIX_EPOCH + Duration::new(1_385_512_440, 123);
        assert_eq!(SystemTime::from(Date::from(after)), after);

        let before = SystemTime::UNIX_EPOCH - Duration::new(86_400, 250_000_000);
        let date = Date::from(before);
        assert_eq!(date.unix_parts(), (-86_401, 750_000_000));
        assert_eq!(SystemTime::from(date), before);
    }

    #[test]
    fn ordering_and_hashing_are_instant_based() {
        let earlier = rfc3339("2013-11-27T00:34:00Z");
        let later = rfc3339("2013-11-27T00:34:01Z");
        assert!(earlier < later);
        assert_eq!(rfc3339("2013-11-27T01:34:00+01:00"), earlier);
    }

    #[test]
    fn from_unix_normalizes_negative_nanos() {
        assert_eq!(
            Date::from_unix(0, -500_000_000).unix_parts(),
            (-1, 500_000_000)
        );
        assert_eq!(
            Date::from_unix(1, 1_500_000_000).unix_parts(),
            (2, 500_000_000)
        );
        assert_eq!(Date::from_unix(i64::MAX, i64::MAX), Date(MAX_INSTANT));
        assert_eq!(Date::from_unix(i64::MIN, i64::MIN), Date(MIN_INSTANT));
    }
}
