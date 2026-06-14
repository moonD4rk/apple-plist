//! The XML plist parser: quick-xml tokens with strict, hand-rolled
//! XML well-formedness checks.

use std::cell::Cell;
use std::{mem, str};

use base64::engine::GeneralPurpose;
use base64::engine::general_purpose::GeneralPurposeConfig;
use base64::{Engine as _, alphabet};
use quick_xml::Reader;
use quick_xml::events::{BytesDecl, BytesStart, Event};

use crate::date::Date;
use crate::depth::DepthGuard;
use crate::error::{Error, Result};
use crate::scalar;
use crate::value::{Integer, Real, Value, maybe_uid};
use crate::xml::in_character_range;

const FORMAT: &str = "XML";

/// Non-strict standard base64: standard alphabet, padding required,
/// non-canonical trailing bits accepted.
const STD_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

/// Parses one XML plist document; bytes after the first root value are
/// never read.
///
/// # Errors
///
/// Returns [`Error::InvalidPlist`] (format `"XML"`) — the decode ladder's
/// retry signal — when the tokenizer fails before any element or the first
/// element is unrecognized; [`Error::MaxDepthExceeded`] past the nesting
/// cap; and [`Error::Parse`] (format `"XML"`) for every other malformed
/// document.
pub(crate) fn parse(data: &[u8]) -> Result<Value> {
    let depth = Cell::new(0);
    let mut reader = Reader::from_reader(data);
    reader.config_mut().check_comments = true;
    let mut parser = Parser {
        reader,
        depth: &depth,
        recognized: false,
        poison: None,
    };
    parser.parse_document()
}

/// One simplified, validated tokenizer event.
enum Token<'a> {
    Start(BytesStart<'a>),
    Empty(BytesStart<'a>),
    End,
    Text(String),
    Ref(char),
    Markup,
    Eof,
}

/// The container state held in one element-recursion frame.
enum Pending {
    /// `<plist>` completes with its first child's value; later tokens inside
    /// the wrapper are never read.
    Plist,
    Dictionary {
        entries: Vec<(String, Value)>,
        pending_key: Option<String>,
    },
    Array(Vec<Value>),
}

/// One open container during iterative descent, holding its depth level
/// until the frame closes.
struct Frame<'a> {
    pending: Pending,
    _guard: DepthGuard<'a>,
}

/// What a frame does with a finished child value.
enum Accepted {
    /// The frame completed with this value; pop it and pass the value down.
    Closed(Value),
    /// The frame consumed the value and continues reading tokens.
    Continue,
}

impl Frame<'_> {
    fn accept(&mut self, value: Value) -> Result<Accepted> {
        match &mut self.pending {
            Pending::Plist => Ok(Accepted::Closed(value)),
            Pending::Dictionary {
                entries,
                pending_key,
            } => {
                let Some(key) = pending_key.take() else {
                    // Unreachable: a value child is only opened with a key pending.
                    return Err(Error::parse(FORMAT, "missing key in dictionary"));
                };
                entries.push((key, value));
                Ok(Accepted::Continue)
            }
            Pending::Array(values) => {
                values.push(value);
                Ok(Accepted::Continue)
            }
        }
    }
}

/// The iterative element walker's continuation between loop turns.
enum Step<'a> {
    /// Evaluate this element: deliver a leaf or open a container frame.
    Open(BytesStart<'a>, bool),
    /// Read the top frame's next token.
    Advance,
    /// Hand a finished value to the top frame, or out of the loop at depth 0.
    Deliver(Value),
}

struct Parser<'a> {
    reader: Reader<&'a [u8]>,
    depth: &'a Cell<usize>,
    recognized: bool,
    poison: Option<Error>,
}

impl<'a> Parser<'a> {
    fn parse_document(&mut self) -> Result<Value> {
        loop {
            let token = match self.next() {
                Ok(token) => token,
                Err(error) => return Err(into_retry_signal(error)),
            };
            match token {
                Token::Start(element) => return self.parse_element(element, false),
                Token::Empty(element) => return self.parse_element(element, true),
                Token::Eof => {
                    return Err(Error::InvalidPlist {
                        format: FORMAT,
                        source: Some("no xml document found".into()),
                    });
                }
                Token::End | Token::Text(_) | Token::Ref(_) | Token::Markup => {}
            }
        }
    }

    /// Pulls one event, applying the cross-cutting tokenizer checks:
    /// attribute well-formedness, text validation, reference resolution,
    /// declaration version/encoding, and the post-failure sticky error.
    fn next(&mut self) -> Result<Token<'a>> {
        if let Some(poisoned) = self.poison.take() {
            return Err(poisoned);
        }
        let event = self
            .reader
            .read_event()
            .map_err(|error| Error::parse(FORMAT, error))?;
        Ok(match event {
            Event::Start(element) => {
                validate_attributes(&element)?;
                Token::Start(element)
            }
            Event::Empty(element) => {
                validate_attributes(&element)?;
                Token::Empty(element)
            }
            Event::End(_) => Token::End,
            Event::Text(text) => Token::Text(text_chunk(&text.into_inner(), false)?),
            Event::CData(cdata) => Token::Text(text_chunk(&cdata.into_inner(), true)?),
            Event::GeneralRef(reference) => Token::Ref(resolve_reference(&reference.into_inner())?),
            Event::Decl(declaration) => {
                validate_declaration(&declaration)?;
                Token::Markup
            }
            Event::Comment(_) | Event::PI(_) | Event::DocType(_) => Token::Markup,
            Event::Eof => Token::Eof,
        })
    }

    /// Depth-first element evaluation with an explicit frame stack instead of
    /// native recursion, so 128 nesting levels hold heap frames rather than
    /// thread stack and the 256 KiB small-stack bound holds in debug builds.
    /// Token order, depth accounting, and every error site match the
    /// recursive shape exactly.
    fn parse_element(&mut self, element: BytesStart<'a>, empty: bool) -> Result<Value> {
        let mut stack: Vec<Frame<'a>> = Vec::new();
        let mut step = Step::Open(element, empty);
        loop {
            step = match step {
                Step::Open(element, empty) => self.open_step(&mut stack, &element, empty)?,
                Step::Advance => self.advance_step(&mut stack)?,
                Step::Deliver(value) => match stack.last_mut() {
                    None => return Ok(value),
                    Some(frame) => match frame.accept(value)? {
                        Accepted::Closed(value) => {
                            drop(stack.pop());
                            Step::Deliver(value)
                        }
                        Accepted::Continue => Step::Advance,
                    },
                },
            };
        }
    }

    /// Element entry: burn a depth level, then either deliver a leaf value or
    /// push a container frame that keeps the level open.
    fn open_step(
        &mut self,
        stack: &mut Vec<Frame<'a>>,
        element: &BytesStart<'a>,
        empty: bool,
    ) -> Result<Step<'a>> {
        let guard = DepthGuard::enter(self.depth)?;
        let local = element.local_name();
        let value = match local.as_ref() {
            b"plist" => {
                self.recognized = true;
                if empty {
                    return Err(Error::parse(FORMAT, "invalid empty <plist/>"));
                }
                stack.push(Frame {
                    pending: Pending::Plist,
                    _guard: guard,
                });
                return Ok(Step::Advance);
            }
            b"string" => {
                self.recognized = true;
                Value::String(self.element_body(empty)?)
            }
            b"integer" => {
                self.recognized = true;
                let body = self.element_body(empty)?;
                parse_integer(&body)?
            }
            b"real" => {
                self.recognized = true;
                let body = self.element_body(empty)?;
                let parsed =
                    scalar::parse_f64(&body).map_err(|error| Error::parse(FORMAT, error))?;
                Value::Real(Real::from(parsed))
            }
            b"true" | b"false" => {
                self.recognized = true;
                let value = local.as_ref() == b"true";
                if !empty && let Err(failure) = self.skip_subtree() {
                    // The skip error is deferred, not raised here; the stream stays poisoned.
                    self.poison = Some(failure);
                }
                Value::Boolean(value)
            }
            b"date" => {
                self.recognized = true;
                let body = self.element_body(empty)?;
                match Date::parse_rfc3339(&body) {
                    Some(date) => Value::Date(date),
                    None => return Err(Error::parse(FORMAT, format!("invalid date {body}"))),
                }
            }
            b"data" => {
                self.recognized = true;
                let body = self.element_body(empty)?;
                let compact: String = body
                    .chars()
                    .filter(|c| !matches!(c, '\t' | '\n' | ' ' | '\r'))
                    .collect();
                let bytes = STD_BASE64
                    .decode(compact)
                    .map_err(|error| Error::parse(FORMAT, error))?;
                Value::Data(bytes)
            }
            b"dict" => {
                self.recognized = true;
                if !empty {
                    stack.push(Frame {
                        pending: Pending::Dictionary {
                            entries: Vec::new(),
                            pending_key: None,
                        },
                        _guard: guard,
                    });
                    return Ok(Step::Advance);
                }
                maybe_uid(Vec::new(), false)
            }
            b"array" => {
                self.recognized = true;
                if !empty {
                    stack.push(Frame {
                        pending: Pending::Array(Vec::new()),
                        _guard: guard,
                    });
                    return Ok(Step::Advance);
                }
                Value::Array(Vec::new())
            }
            _ => {
                let message = format!(
                    "unknown element {}",
                    String::from_utf8_lossy(local.as_ref())
                );
                if self.recognized {
                    return Err(Error::parse(FORMAT, message));
                }
                return Err(Error::InvalidPlist {
                    format: FORMAT,
                    source: Some(message.into()),
                });
            }
        };
        // A leaf releases its depth level here; container guards live in
        // their frames until the closing tag pops them.
        drop(guard);
        Ok(Step::Deliver(value))
    }

    /// Reads the top frame's next tokens until it needs a child element,
    /// completes, or fails — the loop bodies of the recursive
    /// `parse_plist`/`parse_dict`/`parse_array`.
    fn advance_step(&mut self, stack: &mut Vec<Frame<'a>>) -> Result<Step<'a>> {
        let Some(frame) = stack.last_mut() else {
            // Unreachable: Advance is only produced with an open frame.
            return Err(unexpected_eof());
        };
        match &mut frame.pending {
            Pending::Plist => loop {
                match self.next()? {
                    Token::Start(child) => return Ok(Step::Open(child, false)),
                    Token::Empty(child) => return Ok(Step::Open(child, true)),
                    Token::End => return Err(Error::parse(FORMAT, "invalid empty <plist/>")),
                    Token::Eof => return Err(unexpected_eof()),
                    Token::Text(_) | Token::Ref(_) | Token::Markup => {}
                }
            },
            Pending::Dictionary {
                entries,
                pending_key,
            } => loop {
                let (child, child_empty) = match self.next()? {
                    Token::Start(child) => (child, false),
                    Token::Empty(child) => (child, true),
                    Token::End => {
                        if pending_key.is_some() {
                            return Err(Error::parse(FORMAT, "missing value in dictionary"));
                        }
                        let value = maybe_uid(mem::take(entries), false);
                        drop(stack.pop());
                        return Ok(Step::Deliver(value));
                    }
                    Token::Eof => return Err(unexpected_eof()),
                    Token::Text(_) | Token::Ref(_) | Token::Markup => continue,
                };
                if child.local_name().as_ref() == b"key" {
                    *pending_key = Some(self.element_body(child_empty)?);
                } else if pending_key.is_some() {
                    return Ok(Step::Open(child, child_empty));
                } else {
                    return Err(Error::parse(FORMAT, "missing key in dictionary"));
                }
            },
            Pending::Array(values) => loop {
                match self.next()? {
                    Token::Start(child) => return Ok(Step::Open(child, false)),
                    Token::Empty(child) => return Ok(Step::Open(child, true)),
                    Token::End => {
                        let value = Value::Array(mem::take(values));
                        drop(stack.pop());
                        return Ok(Step::Deliver(value));
                    }
                    Token::Eof => return Err(unexpected_eof()),
                    Token::Text(_) | Token::Ref(_) | Token::Markup => {}
                }
            },
        }
    }

    /// Collects an element's character data: concatenates text, CDATA, and
    /// resolved references; skips child elements whole; comments and
    /// processing instructions contribute nothing.
    fn element_body(&mut self, empty: bool) -> Result<String> {
        let mut body = String::new();
        if empty {
            return Ok(body);
        }
        loop {
            match self.next()? {
                Token::Start(_) => self.skip_subtree()?,
                Token::End => return Ok(body),
                Token::Text(text) => body.push_str(&text),
                Token::Ref(resolved) => body.push(resolved),
                Token::Eof => return Err(unexpected_eof()),
                Token::Empty(_) | Token::Markup => {}
            }
        }
    }

    /// Consumes the balance of an already-opened element, validating every
    /// token on the way.
    fn skip_subtree(&mut self) -> Result<()> {
        let mut open = 1_usize;
        while open > 0 {
            match self.next()? {
                Token::Start(_) => open += 1,
                Token::End => open -= 1,
                Token::Eof => return Err(unexpected_eof()),
                Token::Empty(_) | Token::Text(_) | Token::Ref(_) | Token::Markup => {}
            }
        }
        Ok(())
    }
}

/// Reclassifies a hard tokenizer error raised before the first element as
/// the decode ladder's retry signal, keeping the cause.
fn into_retry_signal(error: Error) -> Error {
    match error {
        Error::Parse { format, source } => Error::InvalidPlist { format, source },
        other => other,
    }
}

fn unexpected_eof() -> Error {
    Error::parse(FORMAT, "unexpected end of input")
}

/// Strict attribute validation: names need `=` and quoted values; duplicates
/// are tolerated; entity references and character ranges are validated.
fn validate_attributes(element: &BytesStart<'_>) -> Result<()> {
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| Error::parse(FORMAT, error))?;
        validate_attribute_value(&attribute.value)?;
    }
    Ok(())
}

fn validate_attribute_value(raw: &[u8]) -> Result<()> {
    let text = str::from_utf8(raw).map_err(|_| invalid_utf8())?;
    let mut rest = text;
    loop {
        match rest.split_once('&') {
            None => return validate_character_range(rest),
            Some((before, after)) => {
                validate_character_range(before)?;
                let Some((name, tail)) = after.split_once(';') else {
                    return Err(Error::parse(FORMAT, "character entity without semicolon"));
                };
                let _ = resolve_reference(name.as_bytes())?;
                rest = tail;
            }
        }
    }
}

fn validate_character_range(text: &str) -> Result<()> {
    for c in text.chars() {
        if !in_character_range(c) {
            return Err(Error::parse(
                FORMAT,
                format!("illegal character code {:#x}", u32::from(c)),
            ));
        }
    }
    Ok(())
}

fn invalid_utf8() -> Error {
    Error::parse(FORMAT, "invalid utf-8 in text")
}

/// Validates and normalizes one raw text chunk: `]]>` is rejected outside
/// CDATA, `\r\n`/`\r` collapse to `\n`, and every rune must sit in the XML
/// character range.
fn text_chunk(raw: &[u8], cdata: bool) -> Result<String> {
    if !cdata && raw.windows(3).any(|window| window == b"]]>") {
        return Err(Error::parse(FORMAT, "unescaped ]]> not in cdata section"));
    }
    let text = str::from_utf8(raw).map_err(|_| invalid_utf8())?;
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let c = if c == '\r' {
            if chars.peek() == Some(&'\n') {
                let _ = chars.next();
            }
            '\n'
        } else {
            c
        };
        if !in_character_range(c) {
            return Err(Error::parse(
                FORMAT,
                format!("illegal character code {:#x}", u32::from(c)),
            ));
        }
        out.push(c);
    }
    Ok(out)
}

/// Resolves one `&name;` reference in strict mode: the five predefined
/// names, `&#DDD;`, or `&#xHHH;` (lowercase `x` only); surrogate code
/// points become U+FFFD; the result must sit in the XML character range.
fn resolve_reference(name: &[u8]) -> Result<char> {
    let resolved = match name {
        b"lt" => '<',
        b"gt" => '>',
        b"amp" => '&',
        b"apos" => '\'',
        b"quot" => '"',
        _ => {
            let Some(numeric) = name.strip_prefix(b"#") else {
                return Err(bad_reference(name));
            };
            let (digits, base) = numeric
                .strip_prefix(b"x")
                .map_or((numeric, 10), |hex| (hex, 16));
            let digits = str::from_utf8(digits).map_err(|_| bad_reference(name))?;
            let code = scalar::parse_u64(digits, base).map_err(|_| bad_reference(name))?;
            let code = u32::try_from(code)
                .ok()
                .filter(|&code| code <= 0x0010_FFFF)
                .ok_or_else(|| bad_reference(name))?;
            char::from_u32(code).unwrap_or('\u{FFFD}')
        }
    };
    if in_character_range(resolved) {
        Ok(resolved)
    } else {
        Err(bad_reference(name))
    }
}

fn bad_reference(name: &[u8]) -> Error {
    Error::parse(
        FORMAT,
        format!(
            "invalid character entity &{};",
            String::from_utf8_lossy(name)
        ),
    )
}

/// The `<?xml?>` checks: a `version` pseudo-attribute must be `1.0` and an
/// `encoding` pseudo-attribute must be UTF-8; either may be absent.
fn validate_declaration(declaration: &BytesDecl<'_>) -> Result<()> {
    if let Ok(version) = declaration.version()
        && version.as_ref() != b"1.0"
    {
        return Err(Error::parse(
            FORMAT,
            format!(
                "unsupported xml version {}",
                String::from_utf8_lossy(&version)
            ),
        ));
    }
    if let Some(Ok(encoding)) = declaration.encoding()
        && !encoding.eq_ignore_ascii_case(b"utf-8")
    {
        return Err(Error::parse(
            FORMAT,
            format!(
                "unsupported document encoding {}",
                String::from_utf8_lossy(&encoding)
            ),
        ));
    }
    Ok(())
}

fn parse_integer(body: &str) -> Result<Value> {
    if body.is_empty() {
        return Err(Error::parse(FORMAT, "invalid empty <integer/>"));
    }
    if let Some(rest) = body.strip_prefix('-') {
        let (digits, base) = unsigned_get_base(rest);
        let signed = scalar::parse_i64(&format!("-{digits}"), base)
            .map_err(|error| Error::parse(FORMAT, error))?;
        Ok(Value::Integer(Integer::Signed(signed)))
    } else {
        let (digits, base) = unsigned_get_base(body);
        let unsigned =
            scalar::parse_u64(digits, base).map_err(|error| Error::parse(FORMAT, error))?;
        Ok(Value::Integer(Integer::Unsigned(unsigned)))
    }
}

/// Selects the integer base: an `0x`/`0X` prefix selects base 16.
fn unsigned_get_base(s: &str) -> (&str, u32) {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .map_or((s, 10), |rest| (rest, 16))
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::panic,
        reason = "test code: unwrap and panic are the assertions"
    )]

    use super::*;
    use crate::uid::Uid;
    use crate::value::Dictionary;

    fn parse_ok(input: &str) -> Value {
        match parse(input.as_bytes()) {
            Ok(value) => value,
            Err(error) => panic!("expected {input:?} to parse: {error}"),
        }
    }

    fn is_retry(input: &[u8]) -> bool {
        matches!(parse(input), Err(Error::InvalidPlist { format: "XML", .. }))
    }

    fn is_hard(input: &[u8]) -> bool {
        matches!(parse(input), Err(Error::Parse { format: "XML", .. }))
    }

    fn dict(entries: &[(&str, Value)]) -> Value {
        Value::Dictionary(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<Dictionary>(),
        )
    }

    #[test]
    fn bare_roots_parse_without_plist_wrapper() {
        assert_eq!(
            parse_ok("<string>Hello</string>"),
            Value::String("Hello".into())
        );
        assert_eq!(parse_ok("<true/>"), Value::Boolean(true));
        assert_eq!(parse_ok("<false/>"), Value::Boolean(false));
        assert_eq!(parse_ok("<data/>"), Value::Data(vec![]));
        assert_eq!(parse_ok("<string/>"), Value::String(String::new()));
        assert_eq!(parse_ok("<string></string>"), Value::String(String::new()));
    }

    #[test]
    fn full_document_with_declaration_and_doctype_parses() {
        let doc = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Name</key><string>Dustin</string></dict></plist>";
        assert_eq!(
            parse_ok(doc),
            dict(&[("Name", Value::String("Dustin".into()))])
        );
    }

    #[test]
    fn leading_chardata_comments_and_bom_are_skipped() {
        assert_eq!(
            parse_ok("garbage <!-- c --> <string>x</string>"),
            Value::String("x".into())
        );
        assert_eq!(
            parse(b"\xEF\xBB\xBF<string>x</string>").unwrap(),
            Value::String("x".into())
        );
    }

    #[test]
    fn nested_plist_unwraps_and_trailing_bytes_are_unread() {
        assert_eq!(
            parse_ok("<plist><plist><string>x</string></plist></plist>"),
            Value::String("x".into())
        );
        assert_eq!(
            parse_ok("<plist><dict><key>test</key><string>value</string></dict></plist>\ntest"),
            dict(&[("test", Value::String("value".into()))])
        );
    }

    #[test]
    fn integer_grammar_is_strict() {
        let cases: &[(&str, Value)] = &[
            ("0x68", Value::Integer(Integer::Unsigned(0x68))),
            ("0X65", Value::Integer(Integer::Unsigned(0x65))),
            ("-0x2a", Value::Integer(Integer::Signed(-42))),
            ("0111", Value::Integer(Integer::Unsigned(111))),
            ("099", Value::Integer(Integer::Unsigned(99))),
            ("-042", Value::Integer(Integer::Signed(-42))),
            (
                "18446744073709551615",
                Value::Integer(Integer::Unsigned(u64::MAX)),
            ),
            (
                "-9223372036854775808",
                Value::Integer(Integer::Signed(i64::MIN)),
            ),
            (
                "0xdeadbeeffacecafe",
                Value::Integer(Integer::Unsigned(0xdead_beef_face_cafe)),
            ),
        ];
        for (body, expected) in cases {
            assert_eq!(
                &parse_ok(&format!("<integer>{body}</integer>")),
                expected,
                "{body}"
            );
        }
        for body in [
            "0x",
            "-0x",
            "+5",
            "1_0",
            " 1",
            "helo",
            "18446744073709551616",
        ] {
            assert!(
                is_hard(format!("<integer>{body}</integer>").as_bytes()),
                "{body}"
            );
        }
    }

    #[test]
    fn real_grammar_accepts_c_style_floats() {
        assert_eq!(
            parse_ok("<real>3.141592653589793</real>"),
            Value::Real(Real::from(std::f64::consts::PI))
        );
        assert_eq!(
            parse_ok("<real>inf</real>"),
            Value::Real(Real::from(f64::INFINITY))
        );
        assert_eq!(
            parse_ok("<real>-INFINITY</real>"),
            Value::Real(Real::from(f64::NEG_INFINITY))
        );
        let Value::Real(nan) = parse_ok("<real>NaN</real>") else {
            panic!("expected a real");
        };
        assert!(nan.value().is_nan());
        assert_eq!(
            parse_ok("<real>1_0</real>"),
            Value::Real(Real::from(10.0_f64))
        );
        assert_eq!(
            parse_ok("<real>0x1p-2</real>"),
            Value::Real(Real::from(0.25_f64))
        );
        // Float parsing: underflow quietly becomes zero, overflow is a range error.
        assert_eq!(
            parse_ok("<real>1e-999</real>"),
            Value::Real(Real::from(0.0_f64))
        );
        for body in ["helo", "+nan", "-nan", "1e999", "", " 1.5"] {
            assert!(is_hard(format!("<real>{body}</real>").as_bytes()), "{body}");
        }
    }

    #[test]
    fn boolean_consumes_garbage_and_truncation_succeeds_at_root() {
        assert_eq!(parse_ok("<true>junk</true>"), Value::Boolean(true));
        assert_eq!(parse_ok("<true><x/></true>"), Value::Boolean(true));
        assert_eq!(parse_ok("<plist><true>"), Value::Boolean(true));
        // The poisoned stream still fails the enclosing container.
        assert!(is_hard(b"<plist><dict><key>a</key><true>"));
    }

    #[test]
    fn date_accepts_the_rfc3339_grammar() {
        assert_eq!(
            parse_ok("<date>2013-11-27T00:34:00Z</date>"),
            Value::Date(Date::parse_rfc3339("2013-11-27T00:34:00Z").unwrap())
        );
        assert_eq!(
            parse_ok("<date>2013-11-27T1:34:00,5Z</date>"),
            Value::Date(Date::parse_rfc3339("2013-11-27T1:34:00.5Z").unwrap())
        );
        for body in [
            "*@&amp;%#helo",
            "2013-02-30T00:34:00Z",
            "2013-11-27T00:34:00z",
            "",
        ] {
            assert!(is_hard(format!("<date>{body}</date>").as_bytes()), "{body}");
        }
    }

    #[test]
    fn data_strips_whitespace_and_decodes_base64() {
        assert_eq!(
            parse_ok("<data>aGVsbG8=</data>"),
            Value::Data(b"hello".to_vec())
        );
        assert_eq!(
            parse_ok("<data>aGVs\n\tbG8=  </data>"),
            Value::Data(b"hello".to_vec())
        );
        assert_eq!(parse_ok("<data>QR==</data>"), Value::Data(vec![0x41]));
        assert_eq!(parse_ok("<data>ABC=</data>"), Value::Data(vec![0x00, 0x10]));
        for body in ["QQ", "Q\u{b}Q==", "QQ==garbage", "*@&amp;%#helo"] {
            assert!(is_hard(format!("<data>{body}</data>").as_bytes()), "{body}");
        }
    }

    #[test]
    fn string_body_assembles_entities_cdata_and_skips_children() {
        assert_eq!(
            parse_ok("<string>&lt;*I3&gt;</string>"),
            Value::String("<*I3>".into())
        );
        assert_eq!(
            parse_ok("<string>&amp;&apos;&quot;&#65;&#x41;</string>"),
            Value::String("&'\"AA".into())
        );
        assert_eq!(
            parse_ok("<string><![CDATA[<raw>&amp;]]></string>"),
            Value::String("<raw>&amp;".into())
        );
        assert_eq!(
            parse_ok("<string>a<!-- c -->b<x>skipped</x>c</string>"),
            Value::String("abc".into())
        );
        assert_eq!(
            parse_ok("<string>&#xD800;</string>"),
            Value::String("\u{FFFD}".into())
        );
        assert_eq!(
            parse_ok("<string>&#169;</string>"),
            Value::String("©".into())
        );
    }

    #[test]
    fn carriage_returns_normalize_except_entity_produced_ones() {
        assert_eq!(
            parse_ok("<string>a\r\nb\rc</string>"),
            Value::String("a\nb\nc".into())
        );
        assert_eq!(
            parse_ok("<string>a&#xD;b</string>"),
            Value::String("a\rb".into())
        );
    }

    #[test]
    fn invalid_references_and_characters_error() {
        for body in [
            "&copy;",
            "&amp",
            "&#X41;",
            "&#0;",
            "&#1;",
            "&#xFFFE;",
            "&#x110000;",
            "&#;",
            "&;",
        ] {
            assert!(
                is_hard(format!("<string>{body}</string>").as_bytes()),
                "{body}"
            );
        }
        assert!(is_hard(b"<string>a]]>b</string>"));
        assert!(is_hard(b"<string>\x01</string>"));
        // Same failures before any element are the retry signal.
        assert!(is_retry(b"a&copy;b <string>x</string>"));
        assert!(is_retry(b"]]> <string>x</string>"));
    }

    #[test]
    fn declaration_version_and_encoding_are_checked() {
        assert!(is_retry(b"<?xml version=\"1.1\"?><string>x</string>"));
        assert!(is_retry(
            b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><string>x</string>"
        ));
        assert_eq!(
            parse_ok("<?xml version=\"1.0\" encoding=\"utf-8\"?><string>x</string>"),
            Value::String("x".into())
        );
        assert_eq!(
            parse_ok("<?xml?><string>x</string>"),
            Value::String("x".into())
        );
    }

    #[test]
    fn dict_collects_pairs_with_last_duplicate_winning() {
        let doc = "<plist><dict><key>key</key><string>value</string><key>key</key><string>second value</string></dict></plist>";
        assert_eq!(
            parse_ok(doc),
            dict(&[("key", Value::String("second value".into()))])
        );
        assert_eq!(
            parse_ok("<dict><key/><string>Hello</string></dict>"),
            dict(&[("", Value::String("Hello".into()))])
        );
        assert_eq!(parse_ok("<dict/>"), dict(&[]));
        assert_eq!(
            parse_ok("<dict>10<key>a</key>9<true/>8</dict>"),
            dict(&[("a", Value::Boolean(true))])
        );
    }

    #[test]
    fn cf_uid_single_integer_pairs_collapse() {
        assert_eq!(
            parse_ok("<dict><key>CF$UID</key><integer>1024</integer></dict>"),
            Value::Uid(Uid::from(1024))
        );
        assert_eq!(
            parse_ok("<dict><key>CF$UID</key><integer>-1</integer></dict>"),
            Value::Uid(Uid::from(u64::MAX))
        );
        // String values and duplicate pairs stay dictionaries in XML.
        assert_eq!(
            parse_ok("<dict><key>CF$UID</key><string>1</string></dict>"),
            dict(&[("CF$UID", Value::String("1".into()))])
        );
        assert_eq!(
            parse_ok(
                "<dict><key>CF$UID</key><integer>1</integer><key>CF$UID</key><integer>2</integer></dict>"
            ),
            dict(&[("CF$UID", Value::Integer(Integer::Unsigned(2)))])
        );
        assert_eq!(
            parse_ok(
                "<dict><key>identifier</key><dict><key>CF$UID</key><integer>1024</integer></dict></dict>"
            ),
            dict(&[("identifier", Value::Uid(Uid::from(1024)))])
        );
    }

    #[test]
    fn array_collects_children() {
        assert_eq!(
            parse_ok("<array><integer>1</integer><string>x</string></array>"),
            Value::Array(vec![
                Value::Integer(Integer::Unsigned(1)),
                Value::String("x".into()),
            ])
        );
        assert_eq!(parse_ok("<array/>"), Value::Array(vec![]));
        assert_eq!(parse_ok("<array></array>"), Value::Array(vec![]));
    }

    #[test]
    fn invalid_xml_plists_table_is_rejected() {
        // InvalidXMLPlists rows 0-18: hard errors.
        let hard: &[&str] = &[
            "<plist version=\"1.0\"><integer>0x</integer></plist>",
            "<plist><doct><key>helo</key><string>helo</string></doct></plist>",
            "<plist><dict><string>helo</string></dict></plist>",
            "<plist><dict><key>helo</key></dict></plist>",
            "<integer>helo</integer>",
            "<integer></integer>",
            "<real>helo</real>",
            "<data>*@&amp;%#helo</data>",
            "<date>*@&amp;%#helo</date>",
            "<plist><integer>10</plist>",
            "<plist><real>10</plist>",
            "<plist><string>10</plist>",
            "<plist><dict>10</plist>",
            "<plist><dict><key>10</plist>",
            "<plist>",
            "<plist><data>",
            "<plist><date>",
            "<plist><array>",
            "<plist/>",
        ];
        for input in hard {
            assert!(is_hard(input.as_bytes()), "{input}");
        }
        // Rows 19-20: the retry signal.
        assert!(is_retry(b"<pl"));
        assert!(is_retry(b"bplist00"));
        // Empty and non-XML inputs are the retry signal too.
        assert!(is_retry(b""));
        assert!(is_retry(b"(1,2,3,4,5)"));
        assert!(is_retry(b"{ a = b; }"));
        assert!(is_retry(b"<abab>"));
        assert!(is_retry(b"<0101>"));
        assert!(is_retry(b"<key>x</key>"));
    }

    #[test]
    fn unknown_elements_after_the_first_are_hard() {
        assert!(is_hard(b"<plist><abab></abab></plist>"));
        assert!(is_hard(b"<plist><String>x</String></plist>"));
        assert!(is_hard(b"<dict><key>a</key><abab/></dict>"));
        assert!(is_hard(b"<array><key>a</key></array>"));
    }

    #[test]
    fn namespace_prefixes_dispatch_on_the_local_name() {
        assert_eq!(
            parse_ok("<x:string>hi</x:string>"),
            Value::String("hi".into())
        );
    }

    #[test]
    fn unusual_testdata_verdicts_match_the_derived_table() {
        // s01-s04, s08, s11 parse; s05 diverges (quick-xml rejects inline
        // directives that the expected behavior skips — ruling R12 soft bar);
        // s06, s07, s09, s10 error.
        let s01 = "<plist version=\"1.0\">\n<dict>\n    <key>copyright</key>\n    <string>&#169;</string>\n</dict>\n</plist>\n";
        assert_eq!(
            parse_ok(s01),
            dict(&[("copyright", Value::String("©".into()))])
        );

        let s02 = "<plist version=\"1.0\">\n<dict>\n    <key>name</key >\n    <string>value</string>\n</dict>\n</plist>\n";
        assert_eq!(
            parse_ok(s02),
            dict(&[("name", Value::String("value".into()))])
        );

        let s03 =
            "<plist>\n<dict>\n    <key></key>\n    <string>value</string>\n</dict>\n</plist>\n";
        assert_eq!(parse_ok(s03), dict(&[("", Value::String("value".into()))]));

        let s04 = "<plist>\n<dict>\n    <key><!-- test --></key>\n    <string>value</string>\n</dict>\n</plist>\n";
        assert_eq!(parse_ok(s04), dict(&[("", Value::String("value".into()))]));

        let s05 = "<plist>\n<dict>\n    <key>test<!test></key>\n    <string>value</string>\n</dict>\n</plist>\n";
        assert!(is_hard(s05.as_bytes()));

        let s06 = "<plist>\n<dict>\n    <key>test&amp</key>\n    <string>value</string>\n</dict>\n</plist>\n";
        assert!(is_hard(s06.as_bytes()));

        let s07 = "<plist Q=\">\n<dict>\n    <key>test</key>\n    <string>value</string>\n</dict>\n</plist>\n";
        assert!(is_retry(s07.as_bytes()));

        let s08 = "<plist>\n<dict>\n    <key>test</key>\n    <string>value</string>\n</dict>\n</plist>\ntest\n";
        assert_eq!(
            parse_ok(s08),
            dict(&[("test", Value::String("value".into()))])
        );

        let s09 = "<!DOCTYPE test \">\n<plist>\n<dict>\n    <key>test</key>\n    <string>value</string>\n</dict>\n</plist>\n";
        assert!(is_retry(s09.as_bytes()));

        let s10 = "<plist>\n<dict>\n    <key>test</key>\n    <string =\">apple</string>\n    <!--<string \">libplist</string><!---->\n</dict>\n</plist>\n";
        assert!(is_hard(s10.as_bytes()));

        let s11 = "<plist>\n<dict>\n    <key>test</key>\n    <string>libxml2</string>\n    <key>test</key>\n    <string>apple</string>\n    <key Q=\">\">test</key>\n    <string>libplist</string>\n</dict>\n</plist>";
        assert_eq!(
            parse_ok(s11),
            dict(&[("test", Value::String("libplist".into()))])
        );
    }

    #[test]
    fn scalar_bodies_with_whitespace_are_hard_errors() {
        assert!(is_hard(b"<integer> 1 </integer>"));
        assert!(is_hard(b"<real> 1.5</real>"));
        assert!(is_hard(b"<date> 2013-11-27T00:34:00Z</date>"));
    }

    #[test]
    fn depth_limit_is_hard_and_named() {
        let mut doc = String::from("<plist>");
        for _ in 0..178 {
            doc.push_str("<array>");
        }
        doc.push_str("<string>too deep</string>");
        assert!(matches!(
            parse(doc.as_bytes()),
            Err(Error::MaxDepthExceeded)
        ));

        // 128 levels parse; 129 fail.
        let nest = |n: usize| {
            let mut doc = String::new();
            for _ in 0..n {
                doc.push_str("<array>");
            }
            doc.push_str("<true/>");
            for _ in 0..n {
                doc.push_str("</array>");
            }
            doc
        };
        assert!(parse(nest(127).as_bytes()).is_ok());
        assert!(matches!(
            parse(nest(128).as_bytes()),
            Err(Error::MaxDepthExceeded)
        ));
    }

    #[test]
    fn attribute_malformations_follow_token_position() {
        assert!(is_retry(b"<plist Q=\">"));
        assert!(is_hard(b"<plist><string =\">x</string></plist>"));
        assert!(is_hard(b"<plist><string a=1>x</string></plist>"));
        assert!(is_hard(b"<plist><string a=\"&bad;\">x</string></plist>"));
        assert_eq!(
            parse_ok("<dict><key Q=\">\">test</key><string>v</string></dict>"),
            dict(&[("test", Value::String("v".into()))])
        );
    }
}
