//! The OpenStep/GNUStep text parser.

use std::borrow::Cow;
use std::mem;

use base64::engine::GeneralPurposeConfig;
use base64::engine::general_purpose::GeneralPurpose;
use base64::{Engine, alphabet};

use crate::date::Date;
use crate::depth::MAX_PARSE_DEPTH;
use crate::error::{Error, Result};
use crate::format::Format;
use crate::scalar;
use crate::text::tables::{BASE64_VALID, GS_QUOTABLE, NEWLINE, WHITESPACE};
use crate::value::{Dictionary, Value, maybe_uid};

/// Non-strict standard base64: standard alphabet, canonical padding
/// required, non-canonical trailing bits accepted.
const STD_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

/// Parses one text plist document, reporting which dialect it used.
///
/// The parser starts in OpenStep mode and flips to GNUStep, irreversibly, on
/// the first `<*` or `<[` literal. Empty, whitespace-only, and comment-only
/// input is an empty dictionary; a string root followed by more input
/// re-parses the whole document as a `.strings` dictionary.
///
/// # Errors
///
/// Returns [`Error::Parse`] (format `"text"`) for every malformed document
/// and [`Error::MaxDepthExceeded`] when values nest more than
/// [`MAX_PARSE_DEPTH`] deep.
pub(crate) fn parse(data: &[u8]) -> Result<(Value, Format)> {
    let input = guess_encoding_and_convert(data)?;
    let mut parser = Parser::new(&input);
    let value = parser.parse_document()?;
    Ok((value, parser.format))
}

/// Decodes UTF-16 code units; well-formed surrogate pairs recombine and
/// unpaired surrogates become U+FFFD.
fn convert_u16(buffer: &[u8], big_endian: bool) -> Result<Cow<'_, str>> {
    if !buffer.len().is_multiple_of(2) {
        return Err(Error::parse("text", "truncated utf16"));
    }
    let mut units = Vec::with_capacity(buffer.len() / 2);
    for pair in buffer.chunks_exact(2) {
        if let &[a, b] = pair {
            units.push(if big_endian {
                u16::from_be_bytes([a, b])
            } else {
                u16::from_le_bytes([a, b])
            });
        }
    }
    Ok(Cow::Owned(
        char::decode_utf16(units)
            .map(|unit| unit.unwrap_or('\u{FFFD}'))
            .collect(),
    ))
}

/// Decodes 8-bit input as UTF-8, falling back to Latin-1 one byte at a time
/// for invalid sequences (ruling R18).
fn decode_8bit(buffer: &[u8]) -> Cow<'_, str> {
    if let Ok(valid) = str::from_utf8(buffer) {
        return Cow::Borrowed(valid);
    }
    let mut out = String::with_capacity(buffer.len());
    let mut rest = buffer;
    loop {
        match str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                return Cow::Owned(out);
            }
            Err(err) => {
                let (valid, invalid) = rest.split_at(err.valid_up_to());
                out.push_str(str::from_utf8(valid).unwrap_or_default());
                let Some((&byte, tail)) = invalid.split_first() else {
                    return Cow::Owned(out);
                };
                out.push(char::from(byte));
                rest = tail;
            }
        }
    }
}

/// Sniffs the input encoding in this exact precedence: UTF-8 BOM, UTF-16
/// BOMs, the `00 XX` / `XX 00` heuristics, then raw 8-bit.
fn guess_encoding_and_convert(buffer: &[u8]) -> Result<Cow<'_, str>> {
    if let Some(rest) = buffer.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Ok(decode_8bit(rest));
    }
    if let (Some(&b0), Some(&b1)) = (buffer.first(), buffer.get(1)) {
        let after_bom = buffer.get(2..).unwrap_or_default();
        if b0 == 0xFE && b1 == 0xFF {
            return convert_u16(after_bom, true);
        }
        if b0 == 0 && b1 != 0 {
            return convert_u16(buffer, true);
        }
        if b0 == 0xFF && b1 == 0xFE {
            return convert_u16(after_bom, false);
        }
        if b0 != 0 && b1 == 0 {
            return convert_u16(buffer, false);
        }
    }
    Ok(decode_8bit(buffer))
}

/// The container state held in a value-parsing recursion frame.
enum Pending {
    Dictionary {
        entries: Vec<(String, Value)>,
        pending_key: Option<String>,
        ignore_eof: bool,
    },
    Array(Vec<Value>),
}

/// One open container during iterative descent. `counted` is whether the
/// frame holds a depth level: the `.strings` implicit dictionary does not,
/// because it is entered as a dictionary directly, bypassing the per-value
/// depth accounting.
struct Frame {
    pending: Pending,
    counted: bool,
}

/// The iterative value walker's continuation between loop turns.
enum Step {
    /// Parse the next value node: depth accounting, then dispatch.
    Open,
    /// Resume the top frame at its next structural point.
    Advance,
    /// Hand a finished value to the top frame, or out of the loop at depth 0.
    Deliver(Value),
}

struct Parser<'a> {
    input: &'a str,
    start: usize,
    pos: usize,
    width: usize,
    depth: usize,
    format: Format,
}

impl<'a> Parser<'a> {
    const fn new(input: &'a str) -> Self {
        Self {
            input,
            start: 0,
            pos: 0,
            width: 0,
            depth: 0,
            format: Format::OpenStep,
        }
    }

    fn rest(&self) -> &'a str {
        self.input.get(self.pos..).unwrap_or_default()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.rest().chars().next();
        self.width = c.map_or(0, char::len_utf8);
        self.pos += self.width;
        c
    }

    const fn backup(&mut self) {
        self.pos -= self.width;
    }

    fn peek(&mut self) -> Option<char> {
        let c = self.next();
        self.backup();
        c
    }

    fn emit(&mut self) -> &'a str {
        let s = self.input.get(self.start..self.pos).unwrap_or_default();
        self.start = self.pos;
        s
    }

    const fn ignore(&mut self) {
        self.start = self.pos;
    }

    const fn empty(&self) -> bool {
        self.start == self.pos
    }

    fn scan_until(&mut self, ch: char) {
        match self.rest().find(ch) {
            Some(offset) => self.pos += offset,
            None => self.pos = self.input.len(),
        }
    }

    fn scan_until_any(&mut self, chars: &[char]) {
        match self.rest().find(chars) {
            Some(offset) => self.pos += offset,
            None => self.pos = self.input.len(),
        }
    }

    fn scan_chars_in_set(&mut self, set: super::tables::CharSet) {
        while self.next().is_some_and(|c| set.contains_char(c)) {}
        self.backup();
    }

    fn scan_chars_not_in_set(&mut self, set: super::tables::CharSet) {
        loop {
            match self.next() {
                None => break,
                Some(c) if set.contains_char(c) => break,
                Some(_) => {}
            }
        }
        self.backup();
    }

    /// Builds the hard text-parse error, with a line/character suffix.
    fn error_at(&self, message: &str) -> Error {
        let prefix = self.input.get(..self.pos).unwrap_or_default();
        let line = prefix.matches('\n').count();
        let character = self.pos - prefix.rfind('\n').map_or(0, |index| index + 1);
        Error::parse(
            "text",
            format!("{message} at line {line} character {character}"),
        )
    }

    fn parse_document(&mut self) -> Result<Value> {
        let value = self.parse_value()?;
        self.skip_whitespace_and_comments()?;
        if self.peek().is_some() {
            if !matches!(value, Value::String(_)) {
                return Err(self.error_at("garbage after end of document"));
            }
            // .strings recovery: re-parse the whole input as an implicit
            // dictionary; depth and the dialect flag deliberately persist.
            self.start = 0;
            self.pos = 0;
            let implicit = Frame {
                pending: Pending::Dictionary {
                    entries: Vec::new(),
                    pending_key: None,
                    ignore_eof: true,
                },
                counted: false,
            };
            return self.run(vec![implicit]);
        }
        Ok(value)
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<()> {
        loop {
            self.scan_chars_in_set(WHITESPACE);
            if self.rest().starts_with("//") {
                self.scan_chars_not_in_set(NEWLINE);
            } else if self.rest().starts_with("/*") {
                let Some(end) = self.rest().find("*/") else {
                    return Err(self.error_at("unexpected eof in block comment"));
                };
                self.pos += end + 2;
            } else {
                break;
            }
        }
        self.ignore();
        Ok(())
    }

    fn parse_value(&mut self) -> Result<Value> {
        self.run(Vec::new())
    }

    /// Depth-first value parsing with an explicit frame stack instead of
    /// native recursion, so 128 nesting levels hold heap frames rather than
    /// thread stack and the 256 KiB small-stack bound holds in debug builds.
    /// Scan order, depth accounting, and every error site match the
    /// recursive shape exactly.
    fn run(&mut self, mut stack: Vec<Frame>) -> Result<Value> {
        let mut step = if stack.is_empty() {
            Step::Open
        } else {
            Step::Advance
        };
        loop {
            step = match step {
                Step::Open => self.open_step(&mut stack)?,
                Step::Advance => self.advance_step(&mut stack)?,
                Step::Deliver(value) => match stack.last_mut() {
                    None => return Ok(value),
                    Some(frame) => self.deliver_step(frame, value)?,
                },
            };
        }
    }

    /// The value-node entry: burn a depth level on every value node
    /// (scalars too), then either deliver a scalar or push a container frame
    /// that keeps the level open until it closes.
    fn open_step(&mut self, stack: &mut Vec<Frame>) -> Result<Step> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err(Error::MaxDepthExceeded);
        }
        self.skip_whitespace_and_comments()?;
        let value = match self.next() {
            None => Value::Dictionary(Dictionary::new()),
            Some('<') => match self.next() {
                Some('*') => {
                    self.format = Format::GnuStep;
                    self.parse_gnustep_value()?
                }
                Some('[') => {
                    self.format = Format::GnuStep;
                    self.parse_gnustep_base64()?
                }
                _ => {
                    self.backup();
                    self.parse_hex_data()?
                }
            },
            Some('"') => Value::String(self.parse_quoted_string()?),
            Some('{') => {
                stack.push(Frame {
                    pending: Pending::Dictionary {
                        entries: Vec::new(),
                        pending_key: None,
                        ignore_eof: false,
                    },
                    counted: true,
                });
                return Ok(Step::Advance);
            }
            Some('(') => {
                stack.push(Frame {
                    pending: Pending::Array(Vec::new()),
                    counted: true,
                });
                return Ok(Step::Advance);
            }
            Some(_) => {
                self.backup();
                Value::String(self.parse_unquoted_string()?)
            }
        };
        // A scalar releases its depth level here; containers release theirs
        // when their frame closes in `advance_step`.
        self.depth -= 1;
        Ok(Step::Deliver(value))
    }

    /// Resumes the top frame: the loop bodies of the recursive
    /// `parse_dictionary`/`parse_array`, paused at each child value.
    fn advance_step(&mut self, stack: &mut Vec<Frame>) -> Result<Step> {
        let Some(frame) = stack.last_mut() else {
            // Unreachable: Advance is only produced with an open frame.
            return Err(self.error_at("unexpected eof"));
        };
        match &mut frame.pending {
            Pending::Dictionary {
                entries,
                pending_key,
                ignore_eof,
            } => {
                self.skip_whitespace_and_comments()?;
                let key = match self.next() {
                    None if *ignore_eof => None,
                    None => return Err(self.error_at("unexpected eof in dictionary")),
                    Some('}') => None,
                    Some('"') => Some(self.parse_quoted_string()?),
                    Some(_) => {
                        self.backup();
                        Some(self.parse_unquoted_string()?)
                    }
                };
                let Some(key) = key else {
                    // Lax UID collapse only while the document is still pure
                    // OpenStep, evaluated at dictionary close (order-dependent
                    // on purpose).
                    let value = mem::take(entries);
                    if frame.counted {
                        self.depth -= 1;
                    }
                    drop(stack.pop());
                    return Ok(Step::Deliver(maybe_uid(
                        value,
                        self.format == Format::OpenStep,
                    )));
                };
                self.skip_whitespace_and_comments()?;
                match self.next() {
                    // .strings shorthand: `{key;}` copies the key as the value.
                    Some(';') => {
                        entries.push((key.clone(), Value::String(key)));
                        Ok(Step::Advance)
                    }
                    Some('=') => {
                        *pending_key = Some(key);
                        Ok(Step::Open)
                    }
                    _ => Err(self.error_at("missing '=' in dictionary")),
                }
            }
            Pending::Array(values) => {
                self.skip_whitespace_and_comments()?;
                match self.next() {
                    None => Err(self.error_at("unexpected eof in array")),
                    Some(')') => {
                        let value = Value::Array(mem::take(values));
                        if frame.counted {
                            self.depth -= 1;
                        }
                        drop(stack.pop());
                        Ok(Step::Deliver(value))
                    }
                    Some(',') => Ok(Step::Advance),
                    Some(_) => {
                        self.backup();
                        Ok(Step::Open)
                    }
                }
            }
        }
    }

    /// Hands a finished value to the top frame: dictionaries consume their
    /// pending key and require the trailing `;`, arrays drop empty strings.
    fn deliver_step(&mut self, frame: &mut Frame, value: Value) -> Result<Step> {
        match &mut frame.pending {
            Pending::Dictionary {
                entries,
                pending_key,
                ..
            } => {
                self.skip_whitespace_and_comments()?;
                if self.next() != Some(';') {
                    return Err(self.error_at("missing ';' in dictionary"));
                }
                let Some(key) = pending_key.take() else {
                    // Unreachable: a value is only opened with a key pending.
                    return Err(self.error_at("missing '=' in dictionary"));
                };
                entries.push((key, value));
                Ok(Step::Advance)
            }
            Pending::Array(values) => {
                // Bug-compatible: empty string elements vanish.
                if !matches!(value, Value::String(ref s) if s.is_empty()) {
                    values.push(value);
                }
                Ok(Step::Advance)
            }
        }
    }

    /// The opening `"` has been consumed.
    fn parse_quoted_string(&mut self) -> Result<String> {
        self.ignore();
        let mut slow_path: Option<String> = None;
        loop {
            self.scan_until_any(&['"', '\\']);
            match self.peek() {
                None => return Err(self.error_at("unexpected eof in quoted string")),
                Some('"') => {
                    let section = self.emit();
                    self.pos += 1;
                    return Ok(slow_path.map_or_else(
                        || section.to_owned(),
                        |mut built| {
                            built.push_str(section);
                            built
                        },
                    ));
                }
                Some(_) => {
                    let section = self.emit();
                    let built = slow_path.get_or_insert_with(String::new);
                    built.push_str(section);
                    let _ = self.next();
                    let escape = self.parse_escape();
                    built.push_str(&escape);
                }
            }
        }
    }

    /// The `\` has been consumed. Unknown escapes drop the backslash and
    /// re-scan the following character as ordinary content.
    fn parse_escape(&mut self) -> String {
        let escaped = match self.next() {
            Some('a') => Some('\u{07}'),
            Some('b') => Some('\u{08}'),
            Some('v') => Some('\u{0B}'),
            Some('f') => Some('\u{0C}'),
            Some('t') => Some('\t'),
            Some('r') => Some('\r'),
            Some('n') => Some('\n'),
            Some('\\') => Some('\\'),
            Some('"') => Some('"'),
            Some('x') => Some(escape_char(self.parse_hex_digits(2))),
            Some('u' | 'U') => Some(escape_char(self.parse_hex_digits(4))),
            Some('0'..='7') => {
                self.backup();
                Some(escape_char(self.parse_octal_digits(3)))
            }
            _ => {
                self.backup();
                None
            }
        };
        self.ignore();
        escaped.map(String::from).unwrap_or_default()
    }

    /// Greedy up to `max` digits, stopping early at the first non-digit;
    /// zero digits yield zero.
    fn parse_hex_digits(&mut self, max: usize) -> u32 {
        self.parse_digits(max, 16, 4)
    }

    fn parse_octal_digits(&mut self, max: usize) -> u32 {
        self.parse_digits(max, 8, 3)
    }

    fn parse_digits(&mut self, max: usize, radix: u32, shift: u32) -> u32 {
        let mut value = 0;
        for _ in 0..max {
            let Some(digit) = self.next().and_then(|c| c.to_digit(radix)) else {
                self.backup();
                break;
            };
            value = (value << shift) | digit;
        }
        value
    }

    fn parse_unquoted_string(&mut self) -> Result<String> {
        self.scan_chars_not_in_set(GS_QUOTABLE);
        let s = self.emit();
        if s.is_empty() {
            return Err(self.error_at("invalid unquoted string"));
        }
        Ok(s.to_owned())
    }

    /// The `<*` has been consumed and the format already flipped.
    fn parse_gnustep_value(&mut self) -> Result<Value> {
        let typ = match self.next() {
            None | Some('>') => return Err(self.error_at("invalid GNUStep extended value")),
            Some(c) => c,
        };
        if !matches!(typ, 'I' | 'R' | 'B' | 'D') {
            return Err(self.error_at(&format!("unknown GNUStep extended value type '{typ}'")));
        }
        if self.peek() == Some('"') {
            let _ = self.next();
        }
        self.ignore();
        self.scan_until('>');
        if self.peek().is_none() {
            return Err(self.error_at("unterminated GNUStep extended value"));
        }
        if self.empty() {
            return Err(self.error_at("empty GNUStep extended value"));
        }
        let mut payload = self.emit();
        let _ = self.next();
        // Malformed-quote tolerance: strip exactly one trailing quote.
        if payload.as_bytes().last() == Some(&b'"') {
            payload = payload.get(..payload.len() - 1).unwrap_or_default();
        }
        match typ {
            'I' => self.parse_gnustep_integer(payload),
            'R' => scalar::parse_f64(payload)
                .map(Value::from)
                .map_err(|cause| Error::parse("text", format!("invalid GNUStep real: {cause}"))),
            'B' => self.parse_gnustep_boolean(payload),
            _ => self.parse_gnustep_date(payload),
        }
    }

    fn parse_gnustep_integer(&self, payload: &str) -> Result<Value> {
        if payload.is_empty() {
            return Err(self.error_at("truncated GNUStep extended value"));
        }
        let parsed = if payload.starts_with('-') {
            scalar::parse_i64(payload, 10).map(Value::from)
        } else {
            scalar::parse_u64(payload, 10).map(Value::from)
        };
        parsed.map_err(|cause| Error::parse("text", format!("invalid GNUStep integer: {cause}")))
    }

    fn parse_gnustep_boolean(&self, payload: &str) -> Result<Value> {
        if payload.is_empty() {
            return Err(self.error_at("truncated GNUStep extended value"));
        }
        Ok(Value::Boolean(payload.as_bytes().first() == Some(&b'Y')))
    }

    fn parse_gnustep_date(&self, payload: &str) -> Result<Value> {
        Date::parse_text_layout(payload)
            .map(Value::Date)
            .ok_or_else(|| self.error_at(&format!("invalid GNUStep date: {payload}")))
    }

    /// The `<[` has been consumed and the format already flipped.
    fn parse_gnustep_base64(&mut self) -> Result<Value> {
        self.ignore();
        self.scan_until(']');
        let payload = self.emit();
        if self.next() != Some(']') {
            return Err(self.error_at("invalid GNUStep base64 data: expected ']'"));
        }
        if self.next() != Some('>') {
            return Err(self.error_at("invalid GNUStep base64 data: expected '>'"));
        }
        let filtered: String = payload
            .chars()
            .filter(|c| BASE64_VALID.contains_char(*c))
            .collect();
        STD_BASE64
            .decode(filtered)
            .map(Value::Data)
            .map_err(|cause| self.error_at(&format!("invalid GNUStep base64 data: {cause}")))
    }

    /// The `<` has been consumed; the next character is not `*` or `[`.
    fn parse_hex_data(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        let mut digits: usize = 0;
        loop {
            let Some(c) = self.next() else {
                return Err(self.error_at("unexpected eof in data"));
            };
            match c {
                '>' => {
                    if !digits.is_multiple_of(2) {
                        return Err(self.error_at("uneven number of hex digits in data"));
                    }
                    self.ignore();
                    return Ok(Value::Data(bytes));
                }
                // Pair-splitting whitespace — deliberately not the WHITESPACE
                // bitmap: VT/FF/BS are excluded, U+2028/U+2029 included.
                ' ' | '\t' | '\n' | '\r' | '\u{2028}' | '\u{2029}' => continue,
                _ => {}
            }
            let Some(nibble) = c.to_digit(16).and_then(|d| u8::try_from(d).ok()) else {
                return Err(self.error_at(&format!("unexpected hex digit '{c}'")));
            };
            if digits.is_multiple_of(2) {
                bytes.push(nibble);
            } else if let Some(last) = bytes.last_mut() {
                *last = (*last << 4) | nibble;
            }
            digits += 1;
        }
    }
}

fn escape_char(value: u32) -> char {
    char::from_u32(value).unwrap_or('\u{FFFD}')
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

    use crate::error::Error;
    use crate::format::Format;
    use crate::text::parse;
    use crate::uid::Uid;
    use crate::value::Value;

    /// Every entry of the invalid-text table, all 38 of them.
    const INVALID_TEXT: &[(&str, &[u8])] = &[
        ("Truncated array", b"("),
        ("Truncated dictionary", b"{a=b;"),
        ("Truncated dictionary 2", b"{"),
        ("Unclosed nested array", b"{0=(/"),
        ("Unclosed dictionary", b"{0=/"),
        (
            "Broken GNUStep data",
            b"(<*I5>,<*I5>,<*I5>,<*I5>,*I16777215>,<*I268435455>,<*I4294967295>,<*I18446744073709551615>,)",
        ),
        ("Truncated nested array", b"{0=(((/"),
        ("Truncated dictionary with comment-like", b"{/"),
        ("Truncated array with comment-like", b"(/"),
        ("Truncated array with empty data", b"(<>"),
        ("Bad Extended Character", "{¬=A;}".as_bytes()),
        ("Missing Equals in Dictionary", b"{\"A\"A;}"),
        ("Missing Semicolon in Dictionary", b"{\"A\"=A}"),
        ("Invalid GNUStep type", b"<*F33>"),
        ("Invalid GNUStep int", b"(<*I>"),
        ("Invalid GNUStep date", b"<*D5>"),
        ("Truncated GNUStep value", b"<*I3"),
        ("Invalid data", b"<EQ>"),
        ("Truncated unicode escape", b"\"\\u231"),
        ("Truncated hex escape", b"\"\\x2"),
        ("Truncated octal escape", b"\"\\02"),
        ("Truncated data", b"<33"),
        ("Uneven data", b"<3>"),
        ("Truncated block comment", b"/* hello"),
        ("Truncated quoted string", b"\"hi"),
        ("Garbage after end of non-string", b"<ab> cde"),
        ("Broken UTF-16", b"\xFE\xFF\x01"),
        ("Truncated GNUStep data", b"<"),
        ("Truncated GNUStep base64 data (missing ])", b"<[33=="),
        ("Truncated GNUStep base64 data (missing >)", b"<[33==]"),
        ("Invalid GNUStep base64 data", b"<[3]>"),
        ("GNUStep extended value with EOF before type", b"<*"),
        ("GNUStep extended value terminated before type", b"<*>"),
        ("Empty GNUStep extended value", b"<*I>"),
        ("Unterminated GNUStep quoted value", b"<*D\"5>"),
        ("Unterminated GNUStep quoted value (EOF)", b"<*D\""),
        ("Poorly-terminated GNUStep quoted value", b"<*D\">"),
        ("Empty GNUStep quoted extended value", b"<*D\"\">"),
    ];

    fn ok(data: &[u8]) -> (Value, Format) {
        parse(data).unwrap()
    }

    fn value_of(data: &[u8]) -> Value {
        ok(data).0
    }

    fn s(v: &str) -> Value {
        Value::from(v)
    }

    fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn nested_dicts(n: usize, innermost: &str) -> Vec<u8> {
        let mut doc = String::new();
        for _ in 0..n {
            doc.push_str("{a=");
        }
        doc.push_str(innermost);
        for _ in 0..n {
            doc.push_str(";}");
        }
        doc.into_bytes()
    }

    #[test]
    fn all_38_invalid_text_inputs_error() {
        assert_eq!(INVALID_TEXT.len(), 38);
        for &(name, data) in INVALID_TEXT {
            let result = parse(data);
            assert!(
                matches!(result, Err(Error::Parse { format: "text", .. })),
                "{name}: expected the hard text parse error"
            );
        }
    }

    #[test]
    fn depth_overflow_uses_the_dedicated_variant_at_the_boundary() {
        assert!(parse(&nested_dicts(127, "x")).is_ok());
        assert!(matches!(
            parse(&nested_dicts(128, "x")),
            Err(Error::MaxDepthExceeded)
        ));
        // The innermost empty dictionary is itself the 128th frame.
        assert!(parse(&nested_dicts(127, "{}")).is_ok());
        assert!(matches!(
            parse(&nested_dicts(128, "{}")),
            Err(Error::MaxDepthExceeded)
        ));
        // Depth-limit pin: the maximum parse depth plus 50 nested dicts.
        assert!(matches!(
            parse(&nested_dicts(178, "x")),
            Err(Error::MaxDepthExceeded)
        ));
    }

    #[test]
    fn format_detection_matches_the_pins() {
        assert_eq!(ok(b"(1,2,3,4,5)").1, Format::OpenStep);
        assert_eq!(ok(b"<abab>").1, Format::OpenStep);
        assert_eq!(ok(b"(1,2,<*I3>)").1, Format::GnuStep);
        assert!(parse(&[0x00]).is_err());
    }

    #[test]
    fn empty_whitespace_and_comment_only_documents_are_empty_dictionaries() {
        for doc in [
            &b""[..],
            b" \n\t",
            b"// just a comment",
            b"/* block */",
            b" /* a */ // b",
        ] {
            let (value, format) = ok(doc);
            assert_eq!(value, dict([]));
            assert_eq!(format, Format::OpenStep);
        }
    }

    #[test]
    fn root_may_be_any_value_and_trailing_garbage_depends_on_root_type() {
        assert_eq!(value_of(b"Hello"), s("Hello"));
        assert_eq!(value_of(b"\"hi there\""), s("hi there"));
        assert_eq!(value_of(b"<ab>"), Value::Data(vec![0xAB]));
        assert!(matches!(
            parse(b"<ab> cde"),
            Err(Error::Parse { format: "text", .. })
        ));
        assert!(parse(b"{a=b;} junk").is_err());
        assert!(parse(b"(a) junk").is_err());
    }

    #[test]
    fn strings_file_recovery_reparses_the_full_input() {
        let legacy = b"\"Key\" = \"Value\";\n\t\t\t\"Key2\" = \"Value2\";";
        assert_eq!(
            value_of(legacy),
            dict([("Key", s("Value")), ("Key2", s("Value2"))])
        );

        let shortcut = b"\"Key\";\n\t\t\t\"Key2\";";
        assert_eq!(
            value_of(shortcut),
            dict([("Key", s("Key")), ("Key2", s("Key2"))])
        );

        // A bare `a=b;` root: `a` parses as a string, the re-parse sees a dict.
        assert_eq!(value_of(b"a=b;"), dict([("a", s("b"))]));
        // EOF is tolerated only at key position inside the re-parse.
        assert!(parse(b"Hello world").is_err());
        // The implicit dictionary still runs the lax UID collapse (a quirk).
        assert_eq!(
            value_of(b"\"CF$UID\" = \"1024\";"),
            Value::Uid(Uid::from(1024))
        );
    }

    #[test]
    fn comments_fixture_decodes_and_comments_hide_only_at_skip_sites() {
        let doc: &[u8] = b"{\n\t\t\t\t\tA=1 /* A is 1 because it is the first letter */;\n\t\t\t\t\tB=2; // B is 2 because comment-to-end-of-line.\n\t\t\t\t\tC=3;\n\t\t\t\t\tS = /not/a/comment/;\n\t\t\t\t\tS2 = /not*a/*comm*en/t;\n\t\t\t\t}";
        assert_eq!(
            value_of(doc),
            dict([
                ("A", s("1")),
                ("B", s("2")),
                ("C", s("3")),
                ("S", s("/not/a/comment/")),
                ("S2", s("/not*a/*comm*en/t")),
            ])
        );
        // A `/` inside <hex> is not a comment opener.
        assert!(parse(b"<6/68>").is_err());
        // Block comments do not nest: the first `*/` closes.
        assert_eq!(value_of(b"/* /* */ x"), s("x"));
    }

    #[test]
    fn escapes_fixture_decodes_exactly() {
        let doc: &[u8] = b"{\n\t\t\t\tW=\"\\w\";\n\t\t\t\tA=\"\\a\";\n\t\t\t\tB=\"\\b\";\n\t\t\t\tV=\"\\v\";\n\t\t\t\tF=\"\\f\";\n\t\t\t\tT=\"\\t\";\n\t\t\t\tR=\"\\r\";\n\t\t\t\tN=\"\\n\";\n\t\t\t\tHex1=\"\\xAB\";\n\t\t\t\tUnicode1=\"\\u00AC\";\n\t\t\t\tUnicode2=\"\\U00AD\";\n\t\t\t\tOctal1=\"\\033\";\n\t\t\t}";
        assert_eq!(
            value_of(doc),
            dict([
                ("W", s("w")),
                ("A", s("\u{7}")),
                ("B", s("\u{8}")),
                ("V", s("\u{B}")),
                ("F", s("\u{C}")),
                ("T", s("\t")),
                ("R", s("\r")),
                ("N", s("\n")),
                ("Hex1", s("\u{AB}")),
                ("Unicode1", s("\u{AC}")),
                ("Unicode2", s("\u{AD}")),
                ("Octal1", s("\u{1B}")),
            ])
        );
    }

    #[test]
    fn escape_digit_scanners_are_greedy_with_early_stop() {
        // Pinned: "\x1\u02\U003\4\0057" => 01 02 03 04 05 '7'.
        assert_eq!(
            value_of(b"\"\\x1\\u02\\U003\\4\\0057\""),
            s("\u{1}\u{2}\u{3}\u{4}\u{5}7")
        );
        // Pinned: case-insensitive hex digits.
        assert_eq!(value_of(b"\"\\xaB\\uCdEf\""), s("\u{AB}\u{CDEF}"));
        // Zero digits are tolerated: `\x` + non-hex yields U+0000.
        assert_eq!(value_of(b"\"\\xg\""), s("\u{0}g"));
        assert_eq!(value_of(b"\"\\ug\""), s("\u{0}g"));
    }

    #[test]
    fn surrogate_escapes_never_recombine() {
        assert_eq!(value_of(b"\"\\Ud83d\\Ude00\""), s("\u{FFFD}\u{FFFD}"));
        assert_eq!(value_of(b"\"\\Ud800\""), s("\u{FFFD}"));
    }

    #[test]
    fn quoted_string_content_rules() {
        assert_eq!(value_of(b"\"\""), s(""));
        assert_eq!(value_of(b"\"a\nb\tc\0d\""), s("a\nb\tc\0d"));
        // Unknown escapes drop the backslash and keep the character.
        assert_eq!(value_of(b"\"\\w\\q\""), s("wq"));
        // A backslash at EOF contributes nothing, then the quote check fails.
        assert!(parse(b"\"abc\\").is_err());
    }

    #[test]
    fn unquoted_strings_stop_exactly_at_gs_quotable_members() {
        assert_eq!(value_of("世界".as_bytes()), s("世界"));
        assert_eq!(value_of(b"a-zA-Z0-9$:./_"), s("a-zA-Z0-9$:./_"));
        let (value, _) = ok(b"(1 2)");
        assert_eq!(value, Value::Array(vec![s("1"), s("2")]));
        // U+00AC is <= 0xFF and quotable, so the key scan comes up empty.
        assert!(parse("{¬=A;}".as_bytes()).is_err());
    }

    #[test]
    fn dictionary_shorthand_duplicates_and_errors() {
        assert_eq!(value_of(b"{foo;}"), dict([("foo", s("foo"))]));
        assert_eq!(value_of(b"{Name=Dustin;}"), dict([("Name", s("Dustin"))]));
        assert_eq!(value_of(b"{\"\"=Hello;}"), dict([("", s("Hello"))]));
        // Pinned: duplicate keys are legal, last value wins.
        assert_eq!(
            value_of(b"{\"key\" = \"value\"; \"key\" = \"second value\";}"),
            dict([("key", s("second value"))])
        );
        assert!(parse(b"{a=b}").is_err());
        assert!(parse(b"{a b;}").is_err());
    }

    #[test]
    fn array_comma_tolerance_and_empty_string_dropping() {
        assert_eq!(value_of(b"()"), Value::Array(vec![]));
        assert_eq!(value_of(b"(,a,,b,,)"), Value::Array(vec![s("a"), s("b")]));
        assert_eq!(value_of(b"(a,b,)"), Value::Array(vec![s("a"), s("b")]));
        // Pinned: empty string elements are silently skipped.
        assert_eq!(value_of(b"(A,,,\"\",)"), Value::Array(vec![s("A")]));
        // Only strings vanish; empty data and containers are kept.
        assert_eq!(
            value_of(b"(<>,{},())"),
            Value::Array(vec![Value::Data(vec![]), dict([]), Value::Array(vec![])])
        );
    }

    #[test]
    fn hex_data_grouping_whitespace_and_errors() {
        assert_eq!(value_of(b"<>"), Value::Data(vec![]));
        assert_eq!(value_of(b"<68656c6c 6f>"), Value::Data(b"hello".to_vec()));
        // Whitespace may split a byte's two nibbles; U+2028/U+2029 count.
        assert_eq!(
            value_of("<6\t8 65\r6c\n6c 6\u{2028}f\u{2029}>".as_bytes()),
            Value::Data(b"hello".to_vec())
        );
        // VT is hex-data whitespace in no dialect.
        assert!(parse(b"<3\x0B3>").is_err());
        // The 514-digit unbroken run: grouping spaces are optional on input.
        let mut blob = String::from("<");
        blob.push_str(&"00".repeat(256));
        blob.push_str("01>");
        let mut expected = vec![0u8; 256];
        expected.push(1);
        assert_eq!(value_of(blob.as_bytes()), Value::Data(expected));
    }

    #[test]
    fn gnustep_integer_matrix() {
        assert_eq!(value_of(b"<*I5>"), Value::from(5u64));
        assert_eq!(value_of(b"<*I-5>"), Value::from(-5i64));
        assert_eq!(value_of(b"<*I\"5>"), Value::from(5u64));
        assert_eq!(value_of(b"<*I5\">"), Value::from(5u64));
        assert_eq!(value_of(b"<*I\"1048576\">"), Value::from(1_048_576u64));
        assert_eq!(value_of(b"<*I007>"), Value::from(7u64));
        assert_eq!(value_of(b"<*I18446744073709551615>"), Value::from(u64::MAX));
        assert_eq!(value_of(b"<*I-9223372036854775808>"), Value::from(i64::MIN));
        for bad in [
            &b"<*I+5>"[..],
            b"<*I\"\">",
            b"<*I1\"2>",
            b"<*I18446744073709551616>",
            b"<*I-9223372036854775809>",
            b"<*I5x>",
        ] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn gnustep_real_follows_c_style_parse() {
        assert_eq!(value_of(b"<*R1.5>"), Value::from(1.5));
        assert_eq!(value_of(b"<*R-0.5>"), Value::from(-0.5));
        let nan = value_of(b"<*RNaN>");
        assert!(nan.as_real().unwrap().is_nan());
        assert_eq!(value_of(b"<*R+Inf>"), Value::from(f64::INFINITY));
        assert_eq!(value_of(b"<*R-Inf>"), Value::from(f64::NEG_INFINITY));
        assert_eq!(value_of(b"<*Rinfinity>"), Value::from(f64::INFINITY));
        // Overflow errors; underflow flushes to zero without one (R17).
        assert!(parse(b"<*R1e999>").is_err());
        assert_eq!(value_of(b"<*R1e-999>"), Value::from(0.0));
        // Float-literal exotica accepted: digit-group underscores and C-style
        // hex floats.
        assert_eq!(value_of(b"<*R1_000.5>"), Value::from(1000.5));
        assert_eq!(value_of(b"<*R0x1p-2>"), Value::from(0.25));
        for bad in [&b"<*R+NaN>"[..], b"<*R-nan>", b"<*R>", b"<*Rx>"] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn gnustep_boolean_is_first_byte_y() {
        assert_eq!(value_of(b"<*BY>"), Value::from(true));
        assert_eq!(value_of(b"<*BYES>"), Value::from(true));
        assert_eq!(value_of(b"<*BN>"), Value::from(false));
        assert_eq!(value_of(b"<*BZ>"), Value::from(false));
        assert_eq!(value_of(b"<*Bnope>"), Value::from(false));
        assert_eq!(value_of(b"<*B\"Y>"), Value::from(true));
        assert!(parse(b"<*B\">").is_err());
        assert!(parse(b"<*B\"\">").is_err());
    }

    #[test]
    fn gnustep_date_follows_the_text_layout() {
        let expected = crate::date::Date::parse_text_layout("2013-11-27 00:34:00 +0000").unwrap();
        assert_eq!(
            value_of(b"<*D2013-11-27 00:34:00 +0000>"),
            Value::Date(expected)
        );
        // 1-digit hour and fractional seconds are layout quirks the parser accepts.
        assert!(parse(b"<*D2013-11-27 0:34:00 +0000>").is_ok());
        assert!(parse(b"<*D2013-11-27 00:34:00.25 +0000>").is_ok());
        for bad in [
            &b"<*D2013-11-27T00:34:00 +0000>"[..],
            b"<*D2013-11-27 00:34:00 Z>",
            b"<*D2013-11-27 00:34:00 +00:00>",
            b"<*D2013-02-30 00:34:00 +0000>",
        ] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn quoted_gnustep_values_fixture_decodes() {
        let (value, format) = ok(b"(<*I\"1048576\">, <*I\"1234>, <*B\"Y>)");
        assert_eq!(format, Format::GnuStep);
        assert_eq!(
            value,
            Value::Array(vec![
                Value::from(1_048_576u64),
                Value::from(1234u64),
                Value::from(true),
            ])
        );
    }

    #[test]
    fn gnustep_base64_filters_then_decodes() {
        let (value, format) = ok(b"(<[aGVs^^bG8=]>,<[ a G V s b G 8 = ]>)");
        assert_eq!(format, Format::GnuStep);
        assert_eq!(
            value,
            Value::Array(vec![
                Value::Data(b"hello".to_vec()),
                Value::Data(b"hello".to_vec()),
            ])
        );
        assert_eq!(value_of(b"<[aGVsbG8=]>"), Value::Data(b"hello".to_vec()));
        assert_eq!(value_of(b"<[]>"), Value::Data(vec![]));
        // Non-canonical trailing bits are accepted (non-strict mode)...
        assert_eq!(value_of(b"<[33==]>"), Value::Data(vec![0xDF]));
        // ...but a filtered length of 1 (mod 4) still fails.
        assert!(parse(b"<[3]>").is_err());
        assert!(parse(b"<[aGVsbG8]>").is_err());
    }

    #[test]
    fn uid_collapse_is_lax_only_while_the_document_is_pure_openstep() {
        assert_eq!(value_of(b"{CF$UID=1024;}"), Value::Uid(Uid::from(1024)));
        assert_eq!(value_of(b"{CF$UID=<*I1024>;}"), Value::Uid(Uid::from(1024)));
        // The dict closes while still OpenStep: the string arm applies.
        assert_eq!(
            value_of(b"({CF$UID=255;},<*I1>)"),
            Value::Array(vec![Value::Uid(Uid::from(255)), Value::from(1u64)])
        );
        // After the GNUStep flip the string arm is off; integers still collapse.
        assert_eq!(
            value_of(b"(<*I1>,{CF$UID=255;})"),
            Value::Array(vec![Value::from(1u64), dict([("CF$UID", s("255"))])])
        );
        assert_eq!(
            value_of(b"(<*I1>,{CF$UID=<*I255>;})"),
            Value::Array(vec![Value::from(1u64), Value::Uid(Uid::from(255))])
        );
        // Two raw CF$UID pairs stay a dictionary (single-parsed-pair guard).
        assert_eq!(
            value_of(b"{CF$UID=1;CF$UID=2;}"),
            dict([("CF$UID", s("2"))])
        );
        // Non-numeric string values stay dictionaries even under lax.
        assert_eq!(value_of(b"{CF$UID=x;}"), dict([("CF$UID", s("x"))]));
    }

    #[test]
    fn uid_fixtures_decode_in_both_dialects() {
        let expected = Value::Array(
            [255u64, 65535, 16_777_215, 4_294_967_295, 1_099_511_627_775]
                .into_iter()
                .map(|n| Value::Uid(Uid::from(n)))
                .collect(),
        );
        let os = b"({CF$UID=255;},{CF$UID=65535;},{CF$UID=16777215;},{CF$UID=4294967295;},{CF$UID=1099511627775;},)";
        let gs = b"({CF$UID=<*I255>;},{CF$UID=<*I65535>;},{CF$UID=<*I16777215>;},{CF$UID=<*I4294967295>;},{CF$UID=<*I1099511627775>;},)";
        assert_eq!(ok(os), (expected.clone(), Format::OpenStep));
        assert_eq!(ok(gs), (expected, Format::GnuStep));
    }

    #[test]
    fn encoding_sniff_handles_all_six_pinned_fixtures() {
        let hello = s("Hello");
        assert_eq!(value_of(b"\xEF\xBB\xBFHello"), hello);
        assert_eq!(value_of(b"\xFF\xFEH\x00e\x00l\x00l\x00o\x00"), hello);
        assert_eq!(value_of(b"\xFE\xFF\x00H\x00e\x00l\x00l\x00o"), hello);
        assert_eq!(value_of(b"H\x00e\x00l\x00l\x00o\x00"), hello);
        assert_eq!(value_of(b"\x00H\x00e\x00l\x00l\x00o"), hello);
        let high: &[u8] = &[
            0, b'"', 0, b'H', 0, b'e', 0, b'l', 0, b'l', 0, b'o', 0, b',', 0, b' ', 0x4E, 0x16,
            0x75, 0x4C, 0, b'"',
        ];
        assert_eq!(value_of(high), s("Hello, 世界"));
    }

    #[test]
    fn utf16_surrogates_recombine_but_unpaired_become_replacement() {
        // BE BOM + D83D DE00 (a surrogate pair) inside quotes.
        let paired: &[u8] = &[0xFE, 0xFF, 0x00, b'"', 0xD8, 0x3D, 0xDE, 0x00, 0x00, b'"'];
        assert_eq!(value_of(paired), s("\u{1F600}"));
        let unpaired: &[u8] = &[0xFE, 0xFF, 0x00, b'"', 0xD8, 0x3D, 0x00, b'"'];
        assert_eq!(value_of(unpaired), s("\u{FFFD}"));
    }

    #[test]
    fn encoding_sniff_edge_cases() {
        // `00 00` matches no heuristic and falls through to the 8-bit path,
        // where NUL is quotable: empty unquoted string.
        assert!(parse(&[0x00, 0x00]).is_err());
        // A lone UTF-16 BOM is an empty (even-length) stream: empty dict.
        assert_eq!(value_of(&[0xFE, 0xFF]), dict([]));
        assert_eq!(value_of(&[0xFF, 0xFE]), dict([]));
        // Odd payload after a BOM is truncated UTF-16.
        assert!(matches!(
            parse(&[0xFF, 0xFE, 0x68]),
            Err(Error::Parse { format: "text", .. })
        ));
    }

    #[test]
    fn invalid_utf8_decodes_as_latin1_per_ruling_r18() {
        // C8 alone is invalid UTF-8; Latin-1 maps it to U+00C8 inside quotes.
        assert_eq!(value_of(b"\"\xC8\""), s("\u{C8}"));
        // Mixed: a valid two-byte sequence stays UTF-8.
        assert_eq!(value_of(b"\"\xC3\x88\xC8\""), s("\u{C8}\u{C8}"));
        // Unquoted, a Latin-1 high byte is quotable-class: same verdict as
        // its decoded-UTF-8 twin, unlike the U+FFFD reference (ledgered deviation).
        assert!(parse(b"{\xAC=A;}").is_err());
    }
}
