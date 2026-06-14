//! The `bplist00` generator: preorder flatten with object uniquing.

use std::collections::HashMap;

use crate::date::Date;
use crate::error::{Error, Result};
use crate::value::{Dictionary, Value};

const TAG_BOOL_FALSE: u8 = 0x08;
const TAG_BOOL_TRUE: u8 = 0x09;
const TAG_INTEGER: u8 = 0x10;
const TAG_REAL: u8 = 0x20;
const TAG_DATE: u8 = 0x33;
const TAG_DATA: u8 = 0x40;
const TAG_ASCII_STRING: u8 = 0x50;
const TAG_UTF16_STRING: u8 = 0x60;
const TAG_UID: u8 = 0x80;
const TAG_ARRAY: u8 = 0xA0;
const TAG_DICTIONARY: u8 = 0xD0;

/// Serializes a [`Value`] tree into a complete `bplist00` document.
///
/// Object IDs are assigned in preorder; strings, integers, reals, dates, and
/// data are uniqued (first occurrence wins), while booleans and UIDs follow
/// a last-wins quirk: duplicates stay in the object table as orphans and
/// every reference resolves to the final occurrence. Reals are keyed with
/// IEEE map semantics, so `+0.0` and `-0.0` merge and a `NaN` root encodes
/// (as object 0) while a `NaN` inside a container fails.
///
/// # Errors
///
/// Returns [`Error::Message`] when a container holds a `NaN` real, the one
/// value the uniquing map cannot resolve a reference to.
pub(crate) fn generate(root: &Value) -> Result<Vec<u8>> {
    let mut flattener = Flattener::default();
    let root_slot = flattener.flatten_value(root);
    let Flattener { objmap, objtable } = flattener;

    let num_objects = objtable.len() as u64;
    let object_ref_size = minimum_int_size(num_objects);

    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::with_capacity(objtable.len());
    for entry in &objtable {
        offsets.push(out.len() as u64);
        write_entry(&mut out, entry, object_ref_size, &objmap)?;
    }

    let offset_table_offset = out.len() as u64;
    let offset_int_size = minimum_int_size(offset_table_offset);
    for offset in offsets {
        write_sized_int(&mut out, offset, offset_int_size);
    }

    let top_object = match root_slot {
        RefSlot::Fixed(index) => index,
        // A missed lookup falls back to the map zero value: a NaN root is object 0.
        RefSlot::Find(key) => objmap.get(&key).copied().unwrap_or(0),
        RefSlot::Missing => 0,
    };

    out.extend_from_slice(&[0; 6]);
    out.push(offset_int_size);
    out.push(object_ref_size);
    out.extend_from_slice(&num_objects.to_be_bytes());
    out.extend_from_slice(&top_object.to_be_bytes());
    out.extend_from_slice(&offset_table_offset.to_be_bytes());
    Ok(out)
}

/// The uniquing-map key, one variant per type: raw representation for
/// integers (signed 5 and unsigned 5 stay distinct), zero-normalized bits
/// plus width for reals, and a trusted CRC32 for data (collisions merge
/// silently).
#[derive(Clone, PartialEq, Eq, Hash)]
enum UniqueKey {
    String(String),
    Int { signed: bool, raw: u64 },
    Real { bits: u64, wide: bool },
    Bool(bool),
    Uid(u64),
    Data(u32),
    Date { secs: i64, nanos: u32 },
}

/// How a container resolves one child reference at write time.
enum RefSlot {
    /// A container child: its occurrence's own table index.
    Fixed(u64),
    /// A scalar child: looked up in the final map (first-wins for uniqued
    /// types, last-wins for booleans and UIDs).
    Find(UniqueKey),
    /// A NaN real: the lookup a float-keyed map can never satisfy.
    Missing,
}

enum FlatEntry<'a> {
    String(&'a str),
    Integer {
        signed: bool,
        raw: u64,
    },
    Real {
        value: f64,
        wide: bool,
    },
    Boolean(bool),
    Uid(u64),
    Data(&'a [u8]),
    Date(Date),
    Container {
        tag: u8,
        count: u64,
        refs: Vec<RefSlot>,
    },
}

#[derive(Default)]
struct Flattener<'a> {
    objmap: HashMap<UniqueKey, u64>,
    objtable: Vec<FlatEntry<'a>>,
}

impl<'a> Flattener<'a> {
    fn flatten_value(&mut self, value: &'a Value) -> RefSlot {
        match value {
            Value::Dictionary(dict) => self.flatten_dictionary(dict),
            Value::Array(items) => self.flatten_array(items),
            Value::String(string) => {
                self.flatten_unique(UniqueKey::String(string.clone()), FlatEntry::String(string))
            }
            Value::Integer(integer) => {
                let (signed, raw) = integer.to_raw_parts();
                self.flatten_unique(
                    UniqueKey::Int { signed, raw },
                    FlatEntry::Integer { signed, raw },
                )
            }
            Value::Real(real) => {
                let (value, wide) = (real.value(), real.wide());
                if value.is_nan() {
                    // Every NaN occurrence appends an object the map never finds.
                    self.objtable.push(FlatEntry::Real { value, wide });
                    return RefSlot::Missing;
                }
                self.flatten_unique(
                    UniqueKey::Real {
                        bits: zero_normalized_bits(value),
                        wide,
                    },
                    FlatEntry::Real { value, wide },
                )
            }
            Value::Boolean(boolean) => {
                self.flatten_overwrite(UniqueKey::Bool(*boolean), FlatEntry::Boolean(*boolean))
            }
            Value::Uid(uid) => {
                self.flatten_overwrite(UniqueKey::Uid(uid.get()), FlatEntry::Uid(uid.get()))
            }
            Value::Data(data) => self.flatten_unique(
                UniqueKey::Data(crc32fast::hash(data)),
                FlatEntry::Data(data),
            ),
            Value::Date(date) => {
                let (secs, nanos) = date.unix_parts();
                self.flatten_unique(UniqueKey::Date { secs, nanos }, FlatEntry::Date(*date))
            }
        }
    }

    /// Whitelisted types dedup first-wins: a present key keeps its slot.
    fn flatten_unique(&mut self, key: UniqueKey, entry: FlatEntry<'a>) -> RefSlot {
        if !self.objmap.contains_key(&key) {
            let _ = self.objmap.insert(key.clone(), self.objtable.len() as u64);
            self.objtable.push(entry);
        }
        RefSlot::Find(key)
    }

    /// Booleans and UIDs append unconditionally and overwrite the map entry,
    /// so every reference resolves to the last occurrence (orphans included).
    fn flatten_overwrite(&mut self, key: UniqueKey, entry: FlatEntry<'a>) -> RefSlot {
        let _ = self.objmap.insert(key.clone(), self.objtable.len() as u64);
        self.objtable.push(entry);
        RefSlot::Find(key)
    }

    fn flatten_dictionary(&mut self, dict: &'a Dictionary) -> RefSlot {
        let index = self.objtable.len();
        self.objtable.push(FlatEntry::Container {
            tag: TAG_DICTIONARY,
            count: dict.len() as u64,
            refs: Vec::new(),
        });
        let mut refs = Vec::with_capacity(dict.len() * 2);
        for key in dict.keys() {
            refs.push(self.flatten_unique(UniqueKey::String(key.clone()), FlatEntry::String(key)));
        }
        for value in dict.values() {
            refs.push(self.flatten_value(value));
        }
        self.set_refs(index, refs);
        RefSlot::Fixed(index as u64)
    }

    fn flatten_array(&mut self, items: &'a [Value]) -> RefSlot {
        let index = self.objtable.len();
        self.objtable.push(FlatEntry::Container {
            tag: TAG_ARRAY,
            count: items.len() as u64,
            refs: Vec::new(),
        });
        let refs = items.iter().map(|item| self.flatten_value(item)).collect();
        self.set_refs(index, refs);
        RefSlot::Fixed(index as u64)
    }

    fn set_refs(&mut self, index: usize, refs: Vec<RefSlot>) {
        if let Some(FlatEntry::Container { refs: slot, .. }) = self.objtable.get_mut(index) {
            *slot = refs;
        }
    }
}

fn write_entry(
    out: &mut Vec<u8>,
    entry: &FlatEntry<'_>,
    ref_size: u8,
    objmap: &HashMap<UniqueKey, u64>,
) -> Result<()> {
    match entry {
        FlatEntry::String(string) => write_string(out, string),
        FlatEntry::Integer { signed, raw } => write_int_tag(out, *signed, *raw),
        FlatEntry::Real { value, wide } => write_real(out, *value, *wide),
        FlatEntry::Boolean(boolean) => {
            out.push(if *boolean {
                TAG_BOOL_TRUE
            } else {
                TAG_BOOL_FALSE
            });
        }
        FlatEntry::Uid(value) => write_uid(out, *value),
        FlatEntry::Data(data) => {
            write_counted_tag(out, TAG_DATA, data.len() as u64);
            out.extend_from_slice(data);
        }
        FlatEntry::Date(date) => {
            out.push(TAG_DATE);
            out.extend_from_slice(&date.to_apple_epoch().to_bits().to_be_bytes());
        }
        FlatEntry::Container { tag, count, refs } => {
            write_counted_tag(out, *tag, *count);
            for slot in refs {
                let index = resolve(slot, objmap)?;
                write_sized_int(out, index, ref_size);
            }
        }
    }
    Ok(())
}

fn resolve(slot: &RefSlot, objmap: &HashMap<UniqueKey, u64>) -> Result<u64> {
    match slot {
        RefSlot::Fixed(index) => Ok(*index),
        RefSlot::Find(key) => objmap.get(key).copied().ok_or_else(nan_error),
        RefSlot::Missing => Err(nan_error()),
    }
}

fn nan_error() -> Error {
    Error::Message("nan cannot be uniqued in a binary property list container".to_owned())
}

/// `+0.0` and `-0.0` share one key; the float-keyed map merges them.
const fn zero_normalized_bits(value: f64) -> u64 {
    let bits = value.to_bits();
    if bits << 1 == 0 { 0 } else { bits }
}

/// 1, 2, 4, or 8 — the only widths the generator ever emits.
const fn minimum_int_size(n: u64) -> u8 {
    if n <= 0xFF {
        1
    } else if n <= 0xFFFF {
        2
    } else if n <= 0xFFFF_FFFF {
        4
    } else {
        8
    }
}

fn write_sized_int(out: &mut Vec<u8>, value: u64, size: u8) {
    let bytes = value.to_be_bytes();
    let start = bytes.len().saturating_sub(usize::from(size));
    out.extend_from_slice(bytes.get(start..).unwrap_or(&bytes));
}

/// The integer-tag ladder over the raw u64: unsigned values above
/// `i64::MAX` widen to the 16-byte `0x14` form with a zeroed high half;
/// negatives carry all high bits set and fall through to `0x13`.
fn write_int_tag(out: &mut Vec<u8>, signed: bool, n: u64) {
    if n <= 0xFF {
        out.push(TAG_INTEGER);
        write_sized_int(out, n, 1);
    } else if n <= 0xFFFF {
        out.push(TAG_INTEGER | 0x1);
        write_sized_int(out, n, 2);
    } else if n <= 0xFFFF_FFFF {
        out.push(TAG_INTEGER | 0x2);
        write_sized_int(out, n, 4);
    } else if n > i64::MAX.cast_unsigned() && !signed {
        out.push(TAG_INTEGER | 0x4);
        out.extend_from_slice(&[0; 8]);
        write_sized_int(out, n, 8);
    } else {
        out.push(TAG_INTEGER | 0x3);
        write_sized_int(out, n, 8);
    }
}

/// Inline counts 0–14; 15 and up emit nibble `0xF` plus an integer object.
fn write_counted_tag(out: &mut Vec<u8>, tag: u8, count: u64) {
    if count >= 0xF {
        out.push(tag | 0xF);
        write_int_tag(out, false, count);
    } else {
        out.push(tag | u8::try_from(count).unwrap_or(0));
    }
}

/// Any char above U+007F switches the whole string to UTF-16BE with a
/// code-unit count; otherwise the bytes are written as-is with a byte count.
fn write_string(out: &mut Vec<u8>, string: &str) {
    if string.chars().any(|c| c > '\u{7F}') {
        let units: Vec<u16> = string.encode_utf16().collect();
        write_counted_tag(out, TAG_UTF16_STRING, units.len() as u64);
        for unit in units {
            out.extend_from_slice(&unit.to_be_bytes());
        }
    } else {
        write_counted_tag(out, TAG_ASCII_STRING, string.len() as u64);
        out.extend_from_slice(string.as_bytes());
    }
}

fn write_real(out: &mut Vec<u8>, value: f64, wide: bool) {
    if wide {
        out.push(TAG_REAL | 0x3);
        out.extend_from_slice(&value.to_bits().to_be_bytes());
    } else {
        out.push(TAG_REAL | 0x2);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 narrowing: round-to-nearest-even is the wire contract"
        )]
        let narrow = value as f32;
        out.extend_from_slice(&narrow.to_bits().to_be_bytes());
    }
}

/// Minimum-width payload: markers `0x80`, `0x81`, `0x83`, or `0x87` only.
fn write_uid(out: &mut Vec<u8>, value: u64) {
    let nbytes = minimum_int_size(value);
    out.push(TAG_UID | (nbytes - 1));
    write_sized_int(out, value, nbytes);
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test code: unwrap is the assertion")]

    use super::*;
    use crate::binary::decode_hex;
    use crate::binary::parser::parse;
    use crate::uid::Uid;
    use crate::value::{Integer, Real};

    const STRING_DOC: &str = "62706c69737430305548656c6c6f08000000000000010100000000000000010000000000000000000000000000000e";
    const BOOLEAN_TRUE_DOC: &str =
        "62706c697374303009080000000000000101000000000000000100000000000000000000000000000009";
    const BASIC_STRUCTURE_DOC: &str = "62706c6973743030d10102544e616d655644757374696e080b100000000000000101000000000000000300000000000000000000000000000017";
    const UTF8_STRING_DOC: &str = "62706c6973743030a201025c48656c6c6f2c2041534349496900480065006c006c006f002c00204e16754c080b18000000000000010100000000000000030000000000000000000000000000002b";
    const FLOATS_OF_INCREASING_BITNESS_DOC: &str = "62706c6973743030a20102227f7fffff237fefffffffffffff080b100000000000000101000000000000000300000000000000000000000000000019";
    const DUPLICATED_VALUES_DOC: &str = "62706c6973743030af1010010203040506070208030506010407085548656c6c6f22420000002340400000000000004464617461224280000023405000000000000010643341b8457578000000081b21262f34394244000000000000010100000000000000090000000000000000000000000000004d";
    const UIDS_DOC: &str = "62706c6973743030a5010203040580ff81ffff8300ffffff83ffffffff87000000ffffffffff080e1013181d0000000000000101000000000000000600000000000000000000000000000026";
    const UID_STRUCT_DOC: &str = "62706c6973743030d101025a6964656e746966696572810400080b160000000000000101000000000000000300000000000000000000000000000019";
    const DATE_DOC: &str = "62706c69737430303341b8457578000000080000000000000101000000000000000100000000000000000000000000000011";
    const NAN_DOC: &str = "62706c6973743030237ff8000000000001080000000000000101000000000000000100000000000000000000000000000011";
    const UNSIGNED_LADDER_DOC: &str = "62706c6973743030a901020304050607080910ff110fff11ffff12000fffff1200ffffff120fffffff12ffffffff137fffffffffffffff140000000000000000deadbeeffacecafe081214171a1f24292e370000000000000101000000000000000a00000000000000000000000000000048";
    const SIGNED_INTEGERS_DOC: &str = "62706c6973743030a601020304050613ffffffffffffffff13ffffffffffffff8113ffffffffffffff0113ffffffffffff800113ffffffffffff0001138000000000000000080f18212a333c0000000000000101000000000000000700000000000000000000000000000045";
    const SIXTEEN_ITEMS_DOC: &str = "62706c6973743030af10100102030405060708090a0b0c0d0e0f10100110021003100410051006100710081009100a100b100c100d100e100f1010081b1d1f21232527292b2d2f3133353739000000000000010100000000000000110000000000000000000000000000003b";
    const BLANK_KEY_DOC: &str = "62706c6973743030d10102505548656c6c6f080b0c0000000000000101000000000000000300000000000000000000000000000012";
    const SIZED_INTEGER_BOUNDARIES_DOC: &str = "62706c6973743030a8010203040506070813ffffffffffffff80107f13ffffffffffff8000117fff13ffffffff80000000127fffffff138000000000000000137fffffffffffffff08111a1c252831363f0000000000000101000000000000000900000000000000000000000000000048";

    fn dict(entries: &[(&str, Value)]) -> Value {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn golden_date() -> Date {
        Date::parse_rfc3339("2013-11-27T00:34:00Z").unwrap()
    }

    fn assert_generates(value: &Value, hex: &str) {
        assert_eq!(generate(value).unwrap(), decode_hex(hex));
    }

    #[test]
    fn scalar_goldens_are_byte_exact() {
        assert_generates(&Value::from("Hello"), STRING_DOC);
        assert_generates(&Value::Boolean(true), BOOLEAN_TRUE_DOC);
        assert_generates(&Value::Date(golden_date()), DATE_DOC);
    }

    #[test]
    fn basic_structure_golden_is_byte_exact() {
        assert_generates(
            &dict(&[("Name", Value::from("Dustin"))]),
            BASIC_STRUCTURE_DOC,
        );
    }

    #[test]
    fn ascii_vs_utf16_predicate_is_byte_exact() {
        let value = Value::Array(vec![
            Value::from("Hello, ASCII"),
            Value::from("Hello, \u{4e16}\u{754c}"),
        ]);
        assert_generates(&value, UTF8_STRING_DOC);
    }

    #[test]
    fn narrow_reals_emit_the_0x22_tag() {
        let value = Value::Array(vec![
            Value::from(Real::from(f32::MAX)),
            Value::from(f64::MAX),
        ]);
        assert_generates(&value, FLOATS_OF_INCREASING_BITNESS_DOC);
    }

    #[test]
    fn duplicated_values_unique_first_wins() {
        let date = golden_date();
        let value = Value::Array(vec![
            Value::from("Hello"),
            Value::from(Real::from(32.0f32)),
            Value::from(32.0f64),
            Value::from(b"data".to_vec()),
            Value::from(Real::from(64.0f32)),
            Value::from(64.0f64),
            Value::from(100u64),
            Value::from(Real::from(32.0f32)),
            Value::from(date),
            Value::from(32.0f64),
            Value::from(Real::from(64.0f32)),
            Value::from(64.0f64),
            Value::from("Hello"),
            Value::from(b"data".to_vec()),
            Value::from(100u64),
            Value::from(date),
        ]);
        assert_generates(&value, DUPLICATED_VALUES_DOC);
    }

    #[test]
    fn uid_width_ladder_is_byte_exact() {
        let value = Value::Array(
            [0xFF, 0xFFFF, 0x00FF_FFFF, 0xFFFF_FFFF, 0x00FF_FFFF_FFFF]
                .into_iter()
                .map(|uid| Value::Uid(Uid::from(uid)))
                .collect(),
        );
        assert_generates(&value, UIDS_DOC);
        assert_generates(
            &dict(&[("identifier", Value::Uid(Uid::from(1024)))]),
            UID_STRUCT_DOC,
        );
    }

    #[test]
    fn int_tag_ladder_with_sint128_form_is_byte_exact() {
        let value = Value::Array(vec![
            Value::from(255u64),
            Value::from(4095u64),
            Value::from(65_535u64),
            Value::from(1_048_575u64),
            Value::from(16_777_215u64),
            Value::from(268_435_455u64),
            Value::from(4_294_967_295u64),
            Value::from(9_223_372_036_854_775_807u64),
            Value::from(16_045_690_985_305_262_846u64),
        ]);
        assert_generates(&value, UNSIGNED_LADDER_DOC);
    }

    #[test]
    fn negative_integers_always_use_eight_bytes() {
        let value = Value::Array(
            [-1i64, -127, -255, -32_767, -65_535, i64::MIN]
                .into_iter()
                .map(Value::from)
                .collect(),
        );
        assert_generates(&value, SIGNED_INTEGERS_DOC);
    }

    #[test]
    fn extended_counts_follow_the_int_ladder() {
        let value = Value::Array((1i64..=16).map(Value::from).collect());
        assert_generates(&value, SIXTEEN_ITEMS_DOC);
        // Count 15 is not inlined: nibble 0xF plus an integer object.
        let fifteen = Value::Array((1i64..=15).map(Value::from).collect());
        let generated = generate(&fifteen).unwrap();
        assert_eq!(generated.get(8..11), Some(&[0xAF, 0x10, 0x0F][..]));
    }

    #[test]
    fn empty_ascii_string_is_a_bare_marker() {
        assert_generates(&dict(&[("", Value::from("Hello"))]), BLANK_KEY_DOC);
    }

    #[test]
    fn nan_root_encodes_with_top_object_zero() {
        let nan = f64::from_bits(0x7FF8_0000_0000_0001);
        assert_generates(&Value::from(nan), NAN_DOC);
    }

    #[test]
    fn nan_inside_a_container_fails_to_encode() {
        let nan = Value::from(f64::from_bits(0x7FF8_0000_0000_0001));
        let array = Value::Array(vec![nan.clone()]);
        assert!(matches!(generate(&array), Err(Error::Message(_))));
        let dictionary = dict(&[("k", nan)]);
        assert!(matches!(generate(&dictionary), Err(Error::Message(_))));
    }

    #[test]
    fn positive_and_negative_zero_merge_into_one_object() {
        let expected_for = |first_bits: u64| {
            let mut doc = b"bplist00".to_vec();
            doc.extend_from_slice(&[0xA2, 0x01, 0x01, 0x23]);
            doc.extend_from_slice(&first_bits.to_be_bytes());
            doc.extend_from_slice(&[0x08, 0x0B]);
            doc.extend_from_slice(&[0; 6]);
            doc.extend_from_slice(&[1, 1]);
            doc.extend_from_slice(&2u64.to_be_bytes());
            doc.extend_from_slice(&0u64.to_be_bytes());
            doc.extend_from_slice(&20u64.to_be_bytes());
            doc
        };
        let zeros = Value::Array(vec![Value::from(0.0f64), Value::from(-0.0f64)]);
        assert_eq!(generate(&zeros).unwrap(), expected_for(0));
        let zeros = Value::Array(vec![Value::from(-0.0f64), Value::from(0.0f64)]);
        assert_eq!(
            generate(&zeros).unwrap(),
            expected_for(0x8000_0000_0000_0000)
        );
    }

    #[test]
    fn duplicate_booleans_and_uids_resolve_last_wins_with_orphans() {
        // Array + two true objects; both refs point at the second (index 2).
        let bools = Value::Array(vec![Value::Boolean(true), Value::Boolean(true)]);
        let mut expected = b"bplist00".to_vec();
        expected.extend_from_slice(&[0xA2, 0x02, 0x02, 0x09, 0x09]);
        expected.extend_from_slice(&[0x08, 0x0B, 0x0C]);
        expected.extend_from_slice(&[0; 6]);
        expected.extend_from_slice(&[1, 1]);
        expected.extend_from_slice(&3u64.to_be_bytes());
        expected.extend_from_slice(&0u64.to_be_bytes());
        expected.extend_from_slice(&13u64.to_be_bytes());
        assert_eq!(generate(&bools).unwrap(), expected);

        let uids = Value::Array(vec![Value::Uid(Uid::from(7)), Value::Uid(Uid::from(7))]);
        let mut expected = b"bplist00".to_vec();
        expected.extend_from_slice(&[0xA2, 0x02, 0x02, 0x80, 0x07, 0x80, 0x07]);
        expected.extend_from_slice(&[0x08, 0x0B, 0x0D]);
        expected.extend_from_slice(&[0; 6]);
        expected.extend_from_slice(&[1, 1]);
        expected.extend_from_slice(&3u64.to_be_bytes());
        expected.extend_from_slice(&0u64.to_be_bytes());
        expected.extend_from_slice(&15u64.to_be_bytes());
        assert_eq!(generate(&uids).unwrap(), expected);
    }

    #[test]
    fn signed_and_unsigned_keys_stay_distinct_objects() {
        let value = Value::Array(vec![
            Value::Integer(Integer::Signed(5)),
            Value::Integer(Integer::Unsigned(5)),
        ]);
        let mut expected = b"bplist00".to_vec();
        expected.extend_from_slice(&[0xA2, 0x01, 0x02, 0x10, 0x05, 0x10, 0x05]);
        expected.extend_from_slice(&[0x08, 0x0B, 0x0D]);
        expected.extend_from_slice(&[0; 6]);
        expected.extend_from_slice(&[1, 1]);
        expected.extend_from_slice(&3u64.to_be_bytes());
        expected.extend_from_slice(&0u64.to_be_bytes());
        expected.extend_from_slice(&15u64.to_be_bytes());
        assert_eq!(generate(&value).unwrap(), expected);
    }

    #[test]
    fn data_dedup_trusts_crc32_collisions() {
        // A classic CRC-32 collision pair: different bytes, one table object.
        assert_eq!(crc32fast::hash(b"plumless"), crc32fast::hash(b"buckeroo"));
        let value = Value::Array(vec![
            Value::from(b"plumless".to_vec()),
            Value::from(b"buckeroo".to_vec()),
        ]);
        let generated = generate(&value).unwrap();
        let parsed = parse(&generated).unwrap();
        let expected = Value::Array(vec![
            Value::from(b"plumless".to_vec()),
            Value::from(b"plumless".to_vec()),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn latin1_chars_force_utf16_with_two_byte_offsets() {
        let high_chars: String = (0x80u8..=0xFF).map(char::from).collect();
        let value = dict(&[("_", Value::from(high_chars))]);
        let mut expected = decode_hex("62706c6973743030d10102515f6f1080");
        for unit in 0x0080u16..=0x00FF {
            expected.extend_from_slice(&unit.to_be_bytes());
        }
        expected.extend_from_slice(&[0x00, 0x08, 0x00, 0x0B, 0x00, 0x0D]);
        expected.extend_from_slice(&[0; 6]);
        expected.extend_from_slice(&[2, 1]);
        expected.extend_from_slice(&3u64.to_be_bytes());
        expected.extend_from_slice(&0u64.to_be_bytes());
        expected.extend_from_slice(&0x110u64.to_be_bytes());
        assert_eq!(generate(&value).unwrap(), expected);
    }

    #[test]
    fn cf_uid_dictionaries_never_collapse_in_binary() {
        let value = dict(&[("CF$UID", Value::from(5u64))]);
        let parsed = parse(&generate(&value).unwrap()).unwrap();
        assert_eq!(parsed, value);
        assert!(parsed.as_uid().is_none());
    }

    #[test]
    fn two_hundred_fifty_six_objects_widen_refs_to_two_bytes() {
        let value = Value::Array((0i64..255).map(Value::from).collect());
        let generated = generate(&value).unwrap();
        let ref_size = generated.get(generated.len() - 25).copied().unwrap();
        assert_eq!(ref_size, 2);
        assert_eq!(parse(&generated).unwrap(), value);
    }

    #[test]
    fn empty_containers_round_trip() {
        for value in [
            Value::Array(Vec::new()),
            Value::Dictionary(Dictionary::new()),
            Value::from(""),
            Value::Data(Vec::new()),
        ] {
            let generated = generate(&value).unwrap();
            assert_eq!(parse(&generated).unwrap(), value);
        }
    }

    #[test]
    fn goldens_round_trip_byte_exactly() {
        for hex in [
            STRING_DOC,
            BOOLEAN_TRUE_DOC,
            BASIC_STRUCTURE_DOC,
            UTF8_STRING_DOC,
            FLOATS_OF_INCREASING_BITNESS_DOC,
            DUPLICATED_VALUES_DOC,
            UIDS_DOC,
            UID_STRUCT_DOC,
            DATE_DOC,
            NAN_DOC,
            UNSIGNED_LADDER_DOC,
            SIGNED_INTEGERS_DOC,
            SIXTEEN_ITEMS_DOC,
            BLANK_KEY_DOC,
            SIZED_INTEGER_BOUNDARIES_DOC,
        ] {
            let document = decode_hex(hex);
            let value = parse(&document).unwrap();
            assert_eq!(generate(&value).unwrap(), document);
        }
    }
}
