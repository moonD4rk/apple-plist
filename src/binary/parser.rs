//! The `bplist00` parser: fully buffered, bounds-checked, panic-free.

use crate::date::Date;
use crate::depth::MAX_PARSE_DEPTH;
use crate::error::{Error, Result};
use crate::uid::Uid;
use crate::value::{Dictionary, Integer, Real, Value};

const FORMAT: &str = "binary";
const TRAILER_LEN: usize = 32;
const MIN_DOCUMENT_LEN: usize = 40;
const SIGNED_HIGH_BITS: u64 = u64::MAX;

/// Parses a complete binary property-list document into a [`Value`].
///
/// Wrapping-u8 version arithmetic, the seven trailer checks in order,
/// per-index memoization with per-offset cycle detection, and a
/// container-depth cap of [`MAX_PARSE_DEPTH`].
///
/// # Errors
///
/// Every malformed input returns `Error::Parse { format: "binary" }`, except
/// nesting beyond the cap, which returns [`Error::MaxDepthExceeded`]. Crafted
/// input never panics.
pub(crate) fn parse(buf: &[u8]) -> Result<Value> {
    if buf.len() < MIN_DOCUMENT_LEN {
        return Err(parse_error("not enough data"));
    }
    if buf.get(..6) != Some(b"bplist".as_slice()) {
        return Err(parse_error("incomprehensible magic"));
    }
    let version = wrapped_version(buf);
    if version > 1 {
        return Err(parse_error(format!("unexpected version {version}")));
    }
    let trailer = Trailer::read(buf)?;
    let object_count = usize::try_from(trailer.num_objects).map_err(|_| overflow_error())?;
    let mut parser = Parser {
        buf,
        trailer,
        objects: vec![None; object_count],
        stack: Vec::new(),
    };
    parser.resolve_object(parser.trailer.top_object)
}

/// Exact `((b6-'0')*10)+(b7-'0')` in wrapping u8 arithmetic: non-digit
/// byte pairs that wrap to 0 or 1 are accepted on purpose.
fn wrapped_version(buf: &[u8]) -> u8 {
    let digit = |index: usize| buf.get(index).copied().unwrap_or(0).wrapping_sub(b'0');
    digit(6).wrapping_mul(10).wrapping_add(digit(7))
}

struct Trailer {
    offset_int_size: u8,
    object_ref_size: u8,
    num_objects: u64,
    top_object: u64,
    offset_table_offset: u64,
    /// The byte offset where the 32-byte trailer begins (`len - 32`).
    start: u64,
}

impl Trailer {
    fn read(buf: &[u8]) -> Result<Self> {
        let base = buf.len() - TRAILER_LEN;
        let trailer = Self {
            offset_int_size: buf.get(base + 6).copied().unwrap_or(0),
            object_ref_size: buf.get(base + 7).copied().unwrap_or(0),
            num_objects: be_u64_at(buf, base + 8),
            top_object: be_u64_at(buf, base + 16),
            offset_table_offset: be_u64_at(buf, base + 24),
            start: u64::try_from(base).unwrap_or(u64::MAX),
        };
        trailer.validate()?;
        Ok(trailer)
    }

    /// The seven trailer checks, in their exact order.
    fn validate(&self) -> Result<()> {
        let table = self.offset_table_offset;
        let trailer = self.start;
        let objects = self.num_objects;
        let entry_size = u64::from(self.offset_int_size);

        if table >= trailer {
            return Err(parse_error("offset table begins beyond the trailer"));
        }
        if table < 9 {
            return Err(parse_error("offset table begins inside the header"));
        }
        let Some(table_end) = objects
            .checked_mul(entry_size)
            .and_then(|len| len.checked_add(table))
        else {
            return Err(parse_error("garbage between offset table and trailer"));
        };
        if trailer > table_end {
            return Err(parse_error("garbage between offset table and trailer"));
        }
        if table_end > trailer {
            return Err(parse_error(
                "offset table too short to address every object",
            ));
        }
        // Shift semantics: the count wraps mod 256 in u8, then a u64 shift
        // by 64 or more yields zero instead of being undefined.
        let shift = 8u8.wrapping_mul(self.object_ref_size);
        let max_ref = if shift >= 64 { 0 } else { 1u64 << shift };
        if objects > max_ref {
            return Err(parse_error(
                "more objects than the object reference size can support",
            ));
        }
        if self.offset_int_size < 8 && (1u64 << (8 * u32::from(self.offset_int_size))) <= table {
            return Err(parse_error("offset size cannot address the entire file"));
        }
        if self.top_object >= objects {
            return Err(parse_error("top object out of range"));
        }
        Ok(())
    }
}

/// One open container during iterative resolution: the bookkeeping a
/// recursive resolver would hold per frame plus its container-stack entry.
struct Frame {
    /// The container's byte offset — the cycle-detection key.
    off: u64,
    /// The object-table index to memoize into once the container closes.
    object_index: u64,
    is_dictionary: bool,
    /// Entry count from the marker: pairs for dictionaries, elements for arrays.
    count: u64,
    /// References still expected: `count` or, for dictionaries, `count * 2`.
    total: u64,
    next_ref_pos: u64,
    collected: Vec<Value>,
}

/// The iterative resolver's continuation between loop turns.
enum Step {
    /// Resolve the object at this table index.
    Resolve(u64),
    /// Read the top frame's next reference, or close it when complete.
    Advance,
    /// Hand a finished value to the top frame, or out of the loop at depth 0.
    Deliver(Value),
}

struct Parser<'a> {
    buf: &'a [u8],
    trailer: Trailer,
    objects: Vec<Option<Value>>,
    stack: Vec<Frame>,
}

impl<'a> Parser<'a> {
    /// Depth-first object resolution with an explicit frame stack instead of
    /// native recursion, so 128 nesting levels hold a few hundred bytes of
    /// thread stack and the 256 KiB small-stack bound always holds. Reference
    /// order, memoization timing, and every error site match a recursive
    /// resolve-object/parse-object-list traversal exactly.
    fn resolve_object(&mut self, root: u64) -> Result<Value> {
        let mut step = Step::Resolve(root);
        loop {
            step = match step {
                Step::Resolve(index) => self.resolve_step(index)?,
                Step::Advance => self.advance_step()?,
                Step::Deliver(value) => match self.stack.last_mut() {
                    None => return Ok(value),
                    Some(frame) => {
                        frame.collected.push(value);
                        Step::Advance
                    }
                },
            };
        }
    }

    fn resolve_step(&mut self, index: u64) -> Result<Step> {
        if index >= self.trailer.num_objects {
            return Err(invalid_object_error(index, self.trailer.num_objects));
        }
        let slot = usize::try_from(index).map_err(|_| overflow_error())?;
        if let Some(value) = self.objects.get(slot).and_then(Option::as_ref) {
            return Ok(Step::Deliver(value.clone()));
        }
        let entry_size = u64::from(self.trailer.offset_int_size);
        let entry_pos = index
            .checked_mul(entry_size)
            .and_then(|pos| pos.checked_add(self.trailer.offset_table_offset))
            .ok_or_else(overflow_error)?;
        let (offset, _, _) = self.sized_integer(entry_pos, entry_size)?;
        if offset >= self.trailer.offset_table_offset {
            return Err(beyond_table_error(index));
        }
        let tag = self.slice_at(offset, 1)?.first().copied().unwrap_or(0);
        if matches!(tag & 0xF0, 0xA0 | 0xD0) {
            self.open_container(offset, tag, index)?;
            return Ok(Step::Advance);
        }
        let value = self.parse_scalar_at(offset, tag)?;
        self.memoize(slot, &value);
        Ok(Step::Deliver(value))
    }

    /// Depth check (reject at the cap, before pushing), the per-offset cycle
    /// scan (memoization alone cannot stop self-reference — the memo slot is
    /// written only after a container completes), count, and the whole-list
    /// bounds check, in that exact order.
    fn open_container(&mut self, off: u64, tag: u8, object_index: u64) -> Result<()> {
        if self.stack.len() >= MAX_PARSE_DEPTH {
            return Err(Error::MaxDepthExceeded);
        }
        if self.stack.iter().any(|frame| frame.off == off) {
            return Err(parse_error(format!(
                "self-referential collection at {off:#x}"
            )));
        }
        let (count, start) = self.count_for_tag_at(off)?;
        let is_dictionary = tag & 0xF0 == 0xD0;
        let total = if is_dictionary {
            count.checked_mul(2).ok_or_else(container_error)?
        } else {
            count
        };
        let end = total
            .checked_mul(u64::from(self.trailer.object_ref_size))
            .and_then(|len| start.checked_add(len));
        if end.is_none_or(|end| end > self.trailer.offset_table_offset) {
            return Err(container_error());
        }
        let capacity = usize::try_from(total).map_err(|_| overflow_error())?;
        self.stack.push(Frame {
            off,
            object_index,
            is_dictionary,
            count,
            total,
            next_ref_pos: start,
            collected: Vec::with_capacity(capacity),
        });
        Ok(())
    }

    fn advance_step(&mut self) -> Result<Step> {
        let (complete, next_ref_pos) = match self.stack.last() {
            Some(frame) => (
                u64::try_from(frame.collected.len()).unwrap_or(u64::MAX) >= frame.total,
                frame.next_ref_pos,
            ),
            // Unreachable: Advance is only produced with an open frame.
            None => return Err(overflow_error()),
        };
        if complete {
            let Some(frame) = self.stack.pop() else {
                return Err(overflow_error());
            };
            let value = if frame.is_dictionary {
                build_dictionary(frame.collected, frame.count)?
            } else {
                Value::Array(frame.collected)
            };
            let slot = usize::try_from(frame.object_index).map_err(|_| overflow_error())?;
            self.memoize(slot, &value);
            return Ok(Step::Deliver(value));
        }
        let ref_size = u64::from(self.trailer.object_ref_size);
        let (oid, _, advanced) = self.sized_integer(next_ref_pos, ref_size)?;
        if let Some(frame) = self.stack.last_mut() {
            frame.next_ref_pos = advanced;
        }
        Ok(Step::Resolve(oid))
    }

    fn memoize(&mut self, slot: usize, value: &Value) {
        if let Some(memo) = self.objects.get_mut(slot) {
            *memo = Some(value.clone());
        }
    }

    fn parse_scalar_at(&self, off: u64, tag: u8) -> Result<Value> {
        match tag & 0xF0 {
            0x00 => match tag {
                0x08 => Ok(Value::Boolean(false)),
                0x09 => Ok(Value::Boolean(true)),
                _ => Err(unexpected_atom(tag, off)),
            },
            0x10 => self.parse_integer_at(off, tag),
            0x20 => self.parse_real_at(off, tag),
            0x30 => self.parse_date_at(off),
            0x40 => self.parse_data_at(off).map(Value::Data),
            0x50 => self.parse_ascii_string_at(off).map(Value::String),
            0x60 => self.parse_utf16_string_at(off).map(Value::String),
            0x80 => self.parse_uid_at(off, tag),
            _ => Err(unexpected_atom(tag, off)),
        }
    }

    fn parse_integer_at(&self, off: u64, tag: u8) -> Result<Value> {
        let (lo, hi, _) = self.sized_integer(off.saturating_add(1), 1 << (tag & 0x0F))?;
        Ok(Value::Integer(if hi == SIGNED_HIGH_BITS {
            Integer::Signed(lo.cast_signed())
        } else {
            Integer::Unsigned(lo)
        }))
    }

    /// The low nibble is ignored: every `0x3n` date is an 8-byte f64.
    fn parse_date_at(&self, off: u64) -> Result<Value> {
        let bytes = self.slice_at(off.saturating_add(1), 8)?;
        let seconds = f64::from_bits(be_uint(bytes));
        Ok(Value::Date(Date::from_apple_epoch(seconds)))
    }

    /// UID width is the low nibble plus one, not a power of two.
    fn parse_uid_at(&self, off: u64, tag: u8) -> Result<Value> {
        let nbytes = u64::from(tag & 0x0F) + 1;
        let (lo, _, _) = self.sized_integer(off.saturating_add(1), nbytes)?;
        Ok(Value::Uid(Uid::from(lo)))
    }

    fn parse_real_at(&self, off: u64, tag: u8) -> Result<Value> {
        match 1u32 << (tag & 0x0F) {
            4 => {
                let bytes = self.slice_at(off.saturating_add(1), 4)?;
                let bits = bytes.try_into().map_or(0, u32::from_be_bytes);
                Ok(Value::Real(Real::from(f32::from_bits(bits))))
            }
            8 => {
                let bytes = self.slice_at(off.saturating_add(1), 8)?;
                Ok(Value::Real(Real::from(f64::from_bits(be_uint(bytes)))))
            }
            _ => Err(parse_error("illegal float size")),
        }
    }

    /// Reads a sized integer as `(lo, hi, next_offset)`. Widths 0–7 zero
    /// extend, width 8 sign-extends into `hi` when the top bit is set, width
    /// 16 keeps only the low half plus the high half for the sign test.
    fn sized_integer(&self, off: u64, nbytes: u64) -> Result<(u64, u64, u64)> {
        let (lo, hi) = match nbytes {
            0..=7 => (be_uint(self.slice_at(off, nbytes)?), 0),
            8 => {
                let bytes = self.slice_at(off, 8)?;
                let hi = if bytes.first().is_some_and(|b| b & 0x80 != 0) {
                    SIGNED_HIGH_BITS
                } else {
                    0
                };
                (be_uint(bytes), hi)
            }
            16 => {
                let bytes = self.slice_at(off, 16)?;
                let lo = be_uint(bytes.get(8..).unwrap_or_default());
                let hi = be_uint(bytes.get(..8).unwrap_or_default());
                (lo, hi)
            }
            _ => return Err(parse_error("illegal integer size")),
        };
        Ok((lo, hi, off.saturating_add(nbytes)))
    }

    /// Inline counts 0–14, or an extended-count integer object whose marker
    /// high nibble is ignored. The keystone guard rejects counts beyond the
    /// table offset before any downstream arithmetic can wrap.
    fn count_for_tag_at(&self, off: u64) -> Result<(u64, u64)> {
        let tag = self.slice_at(off, 1)?.first().copied().unwrap_or(0);
        let count = u64::from(tag & 0x0F);
        if count != 0xF {
            return Ok((count, off.saturating_add(1)));
        }
        let marker = self
            .slice_at(off.saturating_add(1), 1)?
            .first()
            .copied()
            .unwrap_or(0);
        let (count, _, next) = self.sized_integer(off.saturating_add(2), 1 << (marker & 0x0F))?;
        if count > self.trailer.offset_table_offset {
            return Err(parse_error("element count exceeds object region"));
        }
        Ok((count, next))
    }

    fn parse_data_at(&self, off: u64) -> Result<Vec<u8>> {
        let (count, start) = self.count_for_tag_at(off)?;
        let bytes = self.counted_payload(start, count, "data exceeds object region")?;
        Ok(bytes.to_vec())
    }

    fn parse_ascii_string_at(&self, off: u64) -> Result<String> {
        let (count, start) = self.count_for_tag_at(off)?;
        let bytes = self.counted_payload(start, count, "string exceeds object region")?;
        // Bytes 0x80..=0xFF decode as Latin-1, never an error (ruling R18).
        Ok(bytes.iter().map(|&byte| char::from(byte)).collect())
    }

    fn parse_utf16_string_at(&self, off: u64) -> Result<String> {
        let message = "utf-16 string exceeds object region";
        let (count, start) = self.count_for_tag_at(off)?;
        let byte_len = count.checked_mul(2).ok_or_else(|| parse_error(message))?;
        let bytes = self.counted_payload(start, byte_len, message)?;
        let units = bytes
            .chunks_exact(2)
            .map(|pair| pair.try_into().map_or(0, u16::from_be_bytes));
        Ok(char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect())
    }

    /// A counted payload must end at or before the offset table; scalar
    /// payloads have no such clamp and may read into the table or trailer.
    fn counted_payload(
        &self,
        start: u64,
        byte_len: u64,
        message: &'static str,
    ) -> Result<&'a [u8]> {
        let end = start
            .checked_add(byte_len)
            .ok_or_else(|| parse_error(message))?;
        if end > self.trailer.offset_table_offset {
            return Err(parse_error(message));
        }
        self.slice_at(start, byte_len)
    }

    fn slice_at(&self, start: u64, len: u64) -> Result<&'a [u8]> {
        let buf_len = u64::try_from(self.buf.len()).unwrap_or(u64::MAX);
        let in_bounds = start
            .checked_add(len)
            .is_some_and(|end| end <= buf_len && start <= buf_len);
        if !in_bounds {
            return Err(self.range_error(start, len));
        }
        let from = usize::try_from(start).map_err(|_| self.range_error(start, len))?;
        let to =
            usize::try_from(start.saturating_add(len)).map_err(|_| self.range_error(start, len))?;
        self.buf
            .get(from..to)
            .ok_or_else(|| self.range_error(start, len))
    }

    fn range_error(&self, start: u64, len: u64) -> Error {
        parse_error(format!(
            "read of {len} bytes at offset {start:#x} exceeds buffer length {}",
            self.buf.len()
        ))
    }
}

/// Big-endian zero-extension of up to eight bytes.
fn be_uint(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |acc, &byte| (acc << 8) | u64::from(byte))
}

fn be_u64_at(buf: &[u8], pos: usize) -> u64 {
    buf.get(pos..pos.saturating_add(8)).map_or(0, be_uint)
}

/// Splits a resolved `[keys..., values...]` object list into a dictionary;
/// every key must be a string, and duplicate keys keep the last value.
fn build_dictionary(objects: Vec<Value>, count: u64) -> Result<Value> {
    let key_count = usize::try_from(count).map_err(|_| overflow_error())?;
    let mut entries = objects.into_iter();
    let mut keys = Vec::with_capacity(key_count);
    for index in 0..key_count {
        match entries.next() {
            Some(Value::String(key)) => keys.push(key),
            Some(_) => {
                return Err(parse_error(format!(
                    "dictionary contains non-string key at index {index}"
                )));
            }
            None => return Err(overflow_error()),
        }
    }
    let dict: Dictionary = keys.into_iter().zip(entries).collect();
    Ok(Value::Dictionary(dict))
}

fn parse_error(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
    Error::parse(FORMAT, message)
}

fn overflow_error() -> Error {
    parse_error("offset arithmetic exceeds document bounds")
}

fn container_error() -> Error {
    parse_error("container exceeds object region")
}

#[cold]
fn invalid_object_error(index: u64, num_objects: u64) -> Error {
    parse_error(format!("invalid object {index} (only {num_objects} exist)"))
}

#[cold]
fn beyond_table_error(index: u64) -> Error {
    parse_error(format!("object {index} starts beyond the offset table"))
}

fn unexpected_atom(tag: u8, off: u64) -> Error {
    parse_error(format!("unexpected atom {tag:#04x} at offset {off:#x}"))
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

    use super::*;
    use crate::binary::decode_hex;

    /// Golden documents (hex of the binary bytes).
    const STRING_DOC: &str = "62706c69737430305548656c6c6f08000000000000010100000000000000010000000000000000000000000000000e";
    const BOOLEAN_TRUE_DOC: &str =
        "62706c697374303009080000000000000101000000000000000100000000000000000000000000000009";
    const BASIC_STRUCTURE_DOC: &str = "62706c6973743030d10102544e616d655644757374696e080b100000000000000101000000000000000300000000000000000000000000000017";
    const DATE_DOC: &str = "62706c69737430303341b8457578000000080000000000000101000000000000000100000000000000000000000000000011";
    const FLOATS_OF_INCREASING_BITNESS_DOC: &str = "62706c6973743030a20102227f7fffff237fefffffffffffff080b100000000000000101000000000000000300000000000000000000000000000019";
    const SIZED_INTEGER_BOUNDARIES_DOC: &str = "62706c6973743030a8010203040506070813ffffffffffffff80107f13ffffffffffff8000117fff13ffffffff80000000127fffffff138000000000000000137fffffffffffffff08111a1c252831363f0000000000000101000000000000000900000000000000000000000000000048";
    const DUPLICATE_DICT_KEYS_DOC: &str = "62706c6973743030d201010203536b65795576616c75655c7365636f6e642076616c7565080d11170000000000000101000000000000000400000000000000000000000000000024";
    /// 16-byte integer truncates to its low half; a non-all-ones high half
    /// is unsigned.
    const INT128_DOC: &str = "62706c6973743030140102030405060708090a0b0c0d0e0f10080000000000000101000000000000000100000000000000000000000000000019";
    /// Non-power-of-two offset integer sizes.
    const THREE_BYTE_OFFSETS_DOC: &str = "62706c6973743030a2010213ffffffffffffff80107f00000800000b0000140000000000000301000000000000000300000000000000000000000000000016";

    /// All 28 invalid-bplist entries, in order.
    const INVALID_DOCS: [&str; 28] = [
        // 00: too short
        "62706c697374303000",
        // 01: bad magic
        "78706c697374303000080000000000000101000000000000000100000000000000000000000000000009",
        // 02: bad version (bplist30)
        "62706c697374333000080000000000000101000000000000000100000000000000000000000000000009",
        // 03: bad version II (bplist@A wraps to 177)
        "62706c697374404100080000000000000101000000000000000100000000000000000000000000000009",
        // 04: offset table inside trailer
        "62706c6973743030000000000000010100000000000000000000000000000000000000000000000a",
        // 05: offset table inside header
        "62706c69737430300000000000000101000000000000000000000000000000000000000000000000",
        // 06: offset table off end of file
        "62706c6973743030000000000000010100000000000000000000000000000000000000000000ff00",
        // 07: garbage between offset table and trailer
        "62706c69737430300009abcd000000000000010100000000000000010000000000000000000000000000000a",
        // 08: top object out of range
        "62706c697374303000080000000000000101000000000000000100000000000000ff0000000000000009",
        // 09: object out of range (offset entry beyond the table)
        "62706c697374303000ff0000000000000101000000000000000100000000000000000000000000000009",
        // 10: object references too small (257 objects, 1-byte refs)
        "62706c69737430300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000101000000000000010100000000000000000000000000000009",
        // 11: offset references too small (table at 0x109, 1-byte offsets)
        "62706c69737430300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000101000000000000000100000000000000000000000000000109",
        // 12: too many objects for the table room
        "62706c69737430300008000000000000010100000000000000ff00000000000000000000000000000009",
        // 13: string way too long (extended count 255 > table offset)
        "62706c69737430305f10ff08000000000000010100000000000000010000000000000000000000000000000b",
        // 14: utf-16 string way too long
        "62706c69737430306f10ff08000000000000010100000000000000010000000000000000000000000000000b",
        // 15: data way too long
        "62706c69737430304f10ff08000000000000010100000000000000010000000000000000000000000000000b",
        // 16: array way too long
        "62706c6973743030af10ff08000000000000010100000000000000010000000000000000000000000000000b",
        // 17: dictionary way too long
        "62706c6973743030df10ff08000000000000010100000000000000010000000000000000000000000000000b",
        // 18: array self-referential
        "62706c6973743030a10008000000000000010100000000000000010000000000000000000000000000000a",
        // 19: dictionary self-referential key
        "62706c6973743030d1000150080b000000000000010100000000000000020000000000000000000000000000000c",
        // 20: dictionary self-referential value
        "62706c6973743030d1010050080b000000000000010100000000000000020000000000000000000000000000000c",
        // 21: dictionary non-string key
        "62706c6973743030d101020809080b0c000000000000010100000000000000030000000000000000000000000000000d",
        // 22: array contains invalid reference
        "62706c6973743030a10f08000000000000010100000000000000010000000000000000000000000000000a",
        // 23: dictionary contains invalid reference
        "62706c6973743030d1010f50080b000000000000010100000000000000020000000000000000000000000000000c",
        // 24: invalid float size (tag 0x27)
        "62706c697374303027080000000000000101000000000000000100000000000000000000000000000009",
        // 25: invalid integer size (tag 0x15)
        "62706c697374303015080000000000000101000000000000000100000000000000000000000000000009",
        // 26: invalid atom (tag 0xFF)
        "62706c6973743030ff080000000000000101000000000000000100000000000000000000000000000009",
        // 27: array refers to itself through a second level
        "62706c6973743030a101a100080a000000000000010100000000000000020000000000000000000000000000000c",
    ];

    fn parse_hex(hex: &str) -> Result<Value> {
        parse(&decode_hex(hex))
    }

    /// Builds a document holding one scalar object with 1-byte offsets/refs.
    fn single_object_doc(object: &[u8]) -> Vec<u8> {
        let mut doc = b"bplist00".to_vec();
        doc.extend_from_slice(object);
        let table = u64::try_from(doc.len()).unwrap();
        doc.push(8);
        doc.extend_from_slice(&[0; 6]);
        doc.push(1);
        doc.push(1);
        doc.extend_from_slice(&1u64.to_be_bytes());
        doc.extend_from_slice(&0u64.to_be_bytes());
        doc.extend_from_slice(&table.to_be_bytes());
        doc
    }

    /// Builds `depth` nested single-element arrays around an ASCII `"x"`,
    /// with 2-byte offsets and refs.
    fn deep_bplist(depth: usize) -> Vec<u8> {
        let mut doc = b"bplist00".to_vec();
        let mut offsets = Vec::with_capacity(depth + 1);
        for level in 0..depth {
            offsets.push(doc.len());
            doc.push(0xA1);
            doc.extend_from_slice(&u16::try_from(level + 1).unwrap().to_be_bytes());
        }
        offsets.push(doc.len());
        doc.extend_from_slice(&[0x51, b'x']);
        let table = u64::try_from(doc.len()).unwrap();
        for offset in &offsets {
            doc.extend_from_slice(&u16::try_from(*offset).unwrap().to_be_bytes());
        }
        doc.extend_from_slice(&[0; 6]);
        doc.push(2);
        doc.push(2);
        doc.extend_from_slice(&u64::try_from(depth + 1).unwrap().to_be_bytes());
        doc.extend_from_slice(&0u64.to_be_bytes());
        doc.extend_from_slice(&table.to_be_bytes());
        doc
    }

    /// An 8-byte extended count of `u64::MAX` that the keystone guard must
    /// reject before any count arithmetic.
    fn overflow_bplist(tag: u8) -> Vec<u8> {
        let mut doc = b"bplist00".to_vec();
        doc.push(tag);
        doc.push(0x13);
        doc.extend_from_slice(&u64::MAX.to_be_bytes());
        doc.push(0x08);
        doc.extend_from_slice(&[0; 6]);
        doc.push(1);
        doc.push(1);
        doc.extend_from_slice(&1u64.to_be_bytes());
        doc.extend_from_slice(&0u64.to_be_bytes());
        doc.extend_from_slice(&0x12u64.to_be_bytes());
        doc
    }

    fn assert_binary_parse_error(result: &Result<Value>) {
        assert!(
            matches!(
                result,
                Err(Error::Parse {
                    format: "binary",
                    ..
                })
            ),
            "expected a binary parse error, got {result:?}"
        );
    }

    fn patch(doc: &mut [u8], pos: usize, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            if let Some(slot) = doc.get_mut(pos + index) {
                *slot = *byte;
            }
        }
    }

    #[test]
    fn parses_the_string_golden() {
        assert_eq!(parse_hex(STRING_DOC).unwrap(), Value::from("Hello"));
    }

    #[test]
    fn parses_the_basic_structure_golden() {
        let expected: Value = std::iter::once(("Name".to_owned(), Value::from("Dustin"))).collect();
        assert_eq!(parse_hex(BASIC_STRUCTURE_DOC).unwrap(), expected);
    }

    #[test]
    fn parses_the_date_golden() {
        let expected = Date::parse_rfc3339("2013-11-27T00:34:00Z").unwrap();
        assert_eq!(parse_hex(DATE_DOC).unwrap(), Value::Date(expected));
    }

    #[test]
    fn int128_truncates_to_the_low_half_unsigned() {
        let parsed = parse_hex(INT128_DOC).unwrap();
        assert_eq!(
            parsed.as_integer(),
            Some(Integer::Unsigned(0x090A_0B0C_0D0E_0F10))
        );

        // Only an all-ones high half marks the value signed.
        let mut object = vec![0x14];
        object.extend_from_slice(&0x8000_0000_0000_0000u64.to_be_bytes());
        object.extend_from_slice(&5u64.to_be_bytes());
        let parsed = parse(&single_object_doc(&object)).unwrap();
        assert_eq!(parsed.as_integer(), Some(Integer::Unsigned(5)));

        let mut object = vec![0x14];
        object.extend_from_slice(&u64::MAX.to_be_bytes());
        object.extend_from_slice(&(-5i64).cast_unsigned().to_be_bytes());
        let parsed = parse(&single_object_doc(&object)).unwrap();
        assert_eq!(parsed.as_integer(), Some(Integer::Signed(-5)));
    }

    #[test]
    fn sized_integer_boundaries_sign_extend_only_eight_byte_values() {
        let parsed = parse_hex(SIZED_INTEGER_BOUNDARIES_DOC).unwrap();
        let integers: Vec<Integer> = parsed
            .into_array()
            .unwrap()
            .iter()
            .map(|v| v.as_integer().unwrap())
            .collect();
        assert_eq!(
            integers,
            [
                Integer::Signed(-128),
                Integer::Unsigned(127),
                Integer::Signed(-32_768),
                Integer::Unsigned(32_767),
                Integer::Signed(-2_147_483_648),
                Integer::Unsigned(2_147_483_647),
                Integer::Signed(i64::MIN),
                Integer::Signed(i64::MAX),
            ]
        );
    }

    #[test]
    fn narrow_and_wide_reals_keep_their_width() {
        let parsed = parse_hex(FLOATS_OF_INCREASING_BITNESS_DOC).unwrap();
        let reals: Vec<(u64, bool)> = parsed
            .into_array()
            .unwrap()
            .iter()
            .map(|v| match v {
                Value::Real(real) => (real.value().to_bits(), real.wide()),
                _ => (0, true),
            })
            .collect();
        assert_eq!(
            reals,
            [
                (f64::from(f32::MAX).to_bits(), false),
                (f64::MAX.to_bits(), true),
            ]
        );
    }

    #[test]
    fn duplicate_dictionary_keys_keep_the_last_value() {
        let parsed = parse_hex(DUPLICATE_DICT_KEYS_DOC).unwrap();
        let expected: Value =
            std::iter::once(("key".to_owned(), Value::from("second value"))).collect();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn version_bytes_use_wrapping_u8_arithmetic() {
        let mut doc = decode_hex(BOOLEAN_TRUE_DOC);
        assert_eq!(parse(&doc).unwrap(), Value::Boolean(true));
        // 0x80 0x10 wraps to version 0 and must be accepted, never "fixed".
        patch(&mut doc, 6, &[0x80, 0x10]);
        assert_eq!(parse(&doc).unwrap(), Value::Boolean(true));
        patch(&mut doc, 6, b"01");
        assert_eq!(parse(&doc).unwrap(), Value::Boolean(true));
        patch(&mut doc, 6, b"30");
        assert_binary_parse_error(&parse(&doc));
    }

    #[test]
    fn all_twenty_eight_invalid_documents_error() {
        for (index, hex) in INVALID_DOCS.iter().enumerate() {
            let result = parse_hex(hex);
            assert!(result.is_err(), "invalid-b-{index:02} unexpectedly parsed");
            assert_binary_parse_error(&result);
        }
    }

    #[test]
    fn crafted_count_overflows_error_for_every_counted_tag() {
        for tag in [0x4F, 0x5F, 0x6F, 0xAF, 0xDF] {
            assert_binary_parse_error(&parse(&overflow_bplist(tag)));
        }
    }

    #[test]
    fn depth_128_parses_and_129_exceeds() {
        assert!(parse(&deep_bplist(MAX_PARSE_DEPTH)).is_ok());
        assert!(matches!(
            parse(&deep_bplist(MAX_PARSE_DEPTH + 1)),
            Err(Error::MaxDepthExceeded)
        ));
        assert!(matches!(
            parse(&deep_bplist(MAX_PARSE_DEPTH + 10)),
            Err(Error::MaxDepthExceeded)
        ));
    }

    #[test]
    fn shallow_nesting_still_parses() {
        let expected = Value::Array(vec![Value::Array(vec![Value::from("x")])]);
        assert_eq!(parse(&deep_bplist(2)).unwrap(), expected);
    }

    #[test]
    fn depth_limit_holds_on_a_small_thread_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                assert!(parse(&deep_bplist(MAX_PARSE_DEPTH)).is_ok());
                assert!(matches!(
                    parse(&deep_bplist(MAX_PARSE_DEPTH + 10)),
                    Err(Error::MaxDepthExceeded)
                ));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn three_byte_offset_entries_parse() {
        let parsed = parse_hex(THREE_BYTE_OFFSETS_DOC).unwrap();
        let expected = Value::Array(vec![
            Value::Integer(Integer::Signed(-128)),
            Value::Integer(Integer::Unsigned(127)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn sixteen_byte_offset_entries_parse() {
        let mut doc = b"bplist00".to_vec();
        doc.push(0x09);
        doc.extend_from_slice(&[0u8; 8]);
        doc.extend_from_slice(&8u64.to_be_bytes());
        doc.extend_from_slice(&[0; 6]);
        doc.push(16);
        doc.push(1);
        doc.extend_from_slice(&1u64.to_be_bytes());
        doc.extend_from_slice(&0u64.to_be_bytes());
        doc.extend_from_slice(&9u64.to_be_bytes());
        assert_eq!(parse(&doc).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn object_ref_size_quirks_follow_shift_semantics() {
        let patch_ref_size = |size: u8| {
            let mut doc = decode_hex(BOOLEAN_TRUE_DOC);
            let pos = doc.len() - 25;
            patch(&mut doc, pos, &[size]);
            doc
        };
        // A scalar root never reads a ref, so a 0-byte ref size parses.
        assert_eq!(parse(&patch_ref_size(0)).unwrap(), Value::Boolean(true));
        // 8 * 8 = shift 64 makes max_ref zero: every document is rejected.
        assert_binary_parse_error(&parse(&patch_ref_size(8)));
        // 8 * 32 wraps to shift 0, so one object passes validation.
        assert_eq!(parse(&patch_ref_size(32)).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn uid_widths_are_low_nibble_plus_one() {
        let parsed = parse(&single_object_doc(&[0x82, 0x01, 0x02, 0x03])).unwrap();
        assert_eq!(parsed, Value::Uid(Uid::from(0x0001_0203)));

        let mut sixteen = vec![0x8F];
        sixteen.extend_from_slice(&[0xAA; 8]);
        sixteen.extend_from_slice(&0xDEAD_BEEFu64.to_be_bytes());
        let parsed = parse(&single_object_doc(&sixteen)).unwrap();
        assert_eq!(parsed, Value::Uid(Uid::from(0xDEAD_BEEF)));

        let mut nine = vec![0x88];
        nine.extend_from_slice(&[0x01; 9]);
        assert_binary_parse_error(&parse(&single_object_doc(&nine)));
    }

    #[test]
    fn every_date_low_nibble_reads_eight_bytes() {
        let expected = Date::parse_rfc3339("2013-11-27T00:34:00Z").unwrap();
        for nibble in [0x30, 0x35, 0x3F] {
            let mut object = vec![nibble];
            object.extend_from_slice(&407_205_240.0f64.to_bits().to_be_bytes());
            assert_eq!(
                parse(&single_object_doc(&object)).unwrap(),
                Value::Date(expected)
            );
        }
    }

    #[test]
    fn ascii_strings_decode_high_bytes_as_latin1() {
        let parsed = parse(&single_object_doc(&[0x52, 0x80, 0xFF])).unwrap();
        assert_eq!(parsed, Value::from("\u{80}\u{ff}"));
    }

    #[test]
    fn utf16_strings_replace_unpaired_surrogates() {
        let parsed = parse(&single_object_doc(&[0x61, 0xD8, 0x00])).unwrap();
        assert_eq!(parsed, Value::from("\u{fffd}"));

        let parsed = parse(&single_object_doc(&[0x62, 0xD8, 0x3D, 0xDE, 0x00])).unwrap();
        assert_eq!(parsed, Value::from("\u{1f600}"));
    }

    #[test]
    fn extended_count_marker_high_nibble_is_ignored() {
        let mut object = vec![0x4F, 0x23];
        object.extend_from_slice(&2u64.to_be_bytes());
        object.extend_from_slice(&[0xAA, 0xBB]);
        let parsed = parse(&single_object_doc(&object)).unwrap();
        assert_eq!(parsed, Value::Data(vec![0xAA, 0xBB]));
    }

    #[test]
    fn counted_payload_may_abut_but_not_cross_the_table() {
        // end == offset_table_offset is legal.
        let parsed = parse(&single_object_doc(&[0x42, 0xAA, 0xBB])).unwrap();
        assert_eq!(parsed, Value::Data(vec![0xAA, 0xBB]));
        // One more byte crosses into the offset table.
        let mut doc = single_object_doc(&[0x42, 0xAA, 0xBB]);
        patch(&mut doc, 8, &[0x43]);
        assert_binary_parse_error(&parse(&doc));
    }

    #[test]
    fn scalar_payloads_may_read_into_table_and_trailer() {
        let parsed = parse(&single_object_doc(&[0x13])).unwrap();
        // The 8-byte payload spans the offset entry and the trailer's start.
        assert_eq!(
            parsed.as_integer(),
            Some(Integer::Unsigned(0x0800_0000_0000_0001))
        );
    }

    #[test]
    fn memoization_fans_out_shared_objects_without_false_cycles() {
        // Array [0x01, 0x01]: the same index twice resolves the object once.
        let mut doc = b"bplist00".to_vec();
        doc.extend_from_slice(&[0xA2, 0x01, 0x01, 0x51, b'x']);
        doc.extend_from_slice(&[8, 11]);
        doc.extend_from_slice(&[0; 6]);
        doc.extend_from_slice(&[1, 1]);
        doc.extend_from_slice(&2u64.to_be_bytes());
        doc.extend_from_slice(&0u64.to_be_bytes());
        doc.extend_from_slice(&13u64.to_be_bytes());
        let expected = Value::Array(vec![Value::from("x"), Value::from("x")]);
        assert_eq!(parse(&doc).unwrap(), expected);

        // Two table entries at one offset are two objects, not a cycle.
        let mut doc = b"bplist00".to_vec();
        doc.extend_from_slice(&[0xA2, 0x01, 0x02, 0x51, b'x']);
        doc.extend_from_slice(&[8, 11, 11]);
        doc.extend_from_slice(&[0; 6]);
        doc.extend_from_slice(&[1, 1]);
        doc.extend_from_slice(&3u64.to_be_bytes());
        doc.extend_from_slice(&0u64.to_be_bytes());
        doc.extend_from_slice(&13u64.to_be_bytes());
        assert_eq!(parse(&doc).unwrap(), expected);
    }

    #[test]
    fn unsupported_atoms_error_with_their_offset() {
        for tag in [0x00, 0x0A, 0x0F, 0x70, 0x90, 0xB0, 0xC0, 0xE0, 0xF0] {
            assert_binary_parse_error(&parse(&single_object_doc(&[tag])));
        }
    }

    #[test]
    fn boolean_markers_parse_without_payload() {
        assert_eq!(
            parse(&single_object_doc(&[0x08])).unwrap(),
            Value::Boolean(false)
        );
        assert_eq!(
            parse(&single_object_doc(&[0x09])).unwrap(),
            Value::Boolean(true)
        );
    }
}
