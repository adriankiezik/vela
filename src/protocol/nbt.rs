//! Binary NBT codec — the on-disk and on-wire tag format.
//!
//! Reference: decompiled `net.minecraft.nbt.*` (MC 26.2). Tag ids and framing
//! are taken from `Tag`, `NbtIo`, and the individual `*Tag` classes; no Mojang
//! code is copied here.
//!
//! Two root framings exist:
//!   * the **named** (disk) variant — `id : u8`, `name : modified-UTF-8`,
//!     `payload` — used for files and the legacy network format; and
//!   * the **nameless** (network) variant used since MC 1.20.2 — `id : u8`,
//!     `payload`, with no root name.
//!
//! Strings use Java's modified UTF-8 (a `u16` byte-length prefix, `0x0000`
//! escaped to two bytes, supplementary code points as CESU-8 surrogate pairs).
//! All multi-byte integers and floats are big-endian.
//!
//! Like `varint.rs`, this operates directly over `bytes::Buf`/`BufMut` so it
//! composes with the rest of the codec rather than extending the packet buffer.

// Foundational codec: registries, text components, and world save/load will
// consume this. Nothing wires it up yet, so silence the unused-item lints.
#![allow(dead_code)]

use std::io::{Error, ErrorKind, Result};

use bytes::{Buf, BufMut};

/// Maximum nesting depth, mirroring vanilla's guard against hostile input that
/// would otherwise blow the stack with deeply nested lists/compounds.
const MAX_DEPTH: u32 = 512;

// Tag ids — see `net.minecraft.nbt.Tag`.
const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// A decoded NBT tag. `Compound` keeps insertion order (a `Vec` of entries)
/// rather than a hash map so re-encoding is deterministic.
#[derive(Debug, Clone, PartialEq)]
pub enum Nbt {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    /// The tag id this value serializes as.
    fn id(&self) -> u8 {
        match self {
            Nbt::End => TAG_END,
            Nbt::Byte(_) => TAG_BYTE,
            Nbt::Short(_) => TAG_SHORT,
            Nbt::Int(_) => TAG_INT,
            Nbt::Long(_) => TAG_LONG,
            Nbt::Float(_) => TAG_FLOAT,
            Nbt::Double(_) => TAG_DOUBLE,
            Nbt::ByteArray(_) => TAG_BYTE_ARRAY,
            Nbt::String(_) => TAG_STRING,
            Nbt::List(_) => TAG_LIST,
            Nbt::Compound(_) => TAG_COMPOUND,
            Nbt::IntArray(_) => TAG_INT_ARRAY,
            Nbt::LongArray(_) => TAG_LONG_ARRAY,
        }
    }
}

// ---------------------------------------------------------------------------
// Ergonomic constructors & accessors
//
// The binary codec above is complete on its own; these helpers exist so the
// builders that compose tags (notably the text-component model in
// `protocol::text`) read naturally and so call sites can introspect a decoded
// compound without matching the enum by hand.
// ---------------------------------------------------------------------------

impl Nbt {
    /// A `TAG_String` from anything string-like.
    pub fn string(s: impl Into<String>) -> Nbt {
        Nbt::String(s.into())
    }

    /// A `TAG_Byte` carrying a boolean (`1`/`0`), the encoding vanilla codecs
    /// use for `Codec.BOOL` over NBT.
    pub fn bool(b: bool) -> Nbt {
        Nbt::Byte(b as i8)
    }

    /// A `TAG_Compound` from an iterator of `(name, tag)` entries, preserving
    /// the iteration order.
    pub fn compound<K: Into<String>, I: IntoIterator<Item = (K, Nbt)>>(entries: I) -> Nbt {
        Nbt::Compound(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Borrow the entries if this is a `Compound`.
    pub fn as_compound(&self) -> Option<&[(String, Nbt)]> {
        match self {
            Nbt::Compound(entries) => Some(entries),
            _ => None,
        }
    }

    /// Borrow the string payload if this is a `String`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Nbt::String(s) => Some(s),
            _ => None,
        }
    }

    /// Look up a child tag by key, if this is a `Compound` containing it.
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        self.as_compound()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a named (disk) root tag: `id`, root `name`, then payload. Returns the
/// root name alongside the tag; an `End` id yields `(String::new(), Nbt::End)`.
pub fn read_named<B: Buf>(buf: &mut B) -> Result<(String, Nbt)> {
    let id = get_u8(buf)?;
    if id == TAG_END {
        return Ok((String::new(), Nbt::End));
    }
    let name = read_modified_utf8(buf)?;
    let tag = read_payload(buf, id, 0)?;
    Ok((name, tag))
}

/// Write a named (disk) root tag: `id`, root `name`, then payload. Writing an
/// `End` emits only the single zero id byte (matching vanilla's framing).
pub fn write_named<B: BufMut>(buf: &mut B, name: &str, tag: &Nbt) {
    buf.put_u8(tag.id());
    if let Nbt::End = tag {
        return;
    }
    write_modified_utf8(buf, name);
    write_payload(buf, tag);
}

/// Read a nameless (network) root tag: `id` then payload, with no root name.
pub fn read_network<B: Buf>(buf: &mut B) -> Result<Nbt> {
    let id = get_u8(buf)?;
    if id == TAG_END {
        return Ok(Nbt::End);
    }
    read_payload(buf, id, 0)
}

/// Write a nameless (network) root tag: `id` then payload, with no root name.
pub fn write_network<B: BufMut>(buf: &mut B, tag: &Nbt) {
    buf.put_u8(tag.id());
    if let Nbt::End = tag {
        return;
    }
    write_payload(buf, tag);
}

// ---------------------------------------------------------------------------
// Payload codec
// ---------------------------------------------------------------------------

fn read_payload<B: Buf>(buf: &mut B, id: u8, depth: u32) -> Result<Nbt> {
    // Vanilla `NbtAccounter.pushDepth` throws at `depth >= maxDepth` (512), so the
    // 512nd level of nesting is rejected — use `>=`, not `>`, to match.
    if depth >= MAX_DEPTH {
        return Err(Error::new(ErrorKind::InvalidData, "NBT nested too deeply"));
    }
    Ok(match id {
        TAG_END => Nbt::End,
        TAG_BYTE => Nbt::Byte(get_i8(buf)?),
        TAG_SHORT => Nbt::Short(get(buf, 2, Buf::get_i16)?),
        TAG_INT => Nbt::Int(get(buf, 4, Buf::get_i32)?),
        TAG_LONG => Nbt::Long(get(buf, 8, Buf::get_i64)?),
        TAG_FLOAT => Nbt::Float(get(buf, 4, Buf::get_f32)?),
        TAG_DOUBLE => Nbt::Double(get(buf, 8, Buf::get_f64)?),
        TAG_BYTE_ARRAY => {
            let len = read_array_len(buf)?;
            let mut v = Vec::with_capacity(cap_hint(buf, len, 1));
            for _ in 0..len {
                v.push(get_i8(buf)?);
            }
            Nbt::ByteArray(v)
        }
        TAG_STRING => Nbt::String(read_modified_utf8(buf)?),
        TAG_LIST => {
            let elem = get_u8(buf)?;
            let len = read_array_len(buf)?;
            if elem == TAG_END && len > 0 {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "non-empty list of TAG_End",
                ));
            }
            // Smallest possible element payload is one byte (e.g. a list of
            // bytes), so bound the hint by the raw remaining byte count.
            let mut v = Vec::with_capacity(cap_hint(buf, len, 1));
            for _ in 0..len {
                v.push(read_payload(buf, elem, depth + 1)?);
            }
            Nbt::List(v)
        }
        TAG_COMPOUND => {
            let mut entries = Vec::new();
            loop {
                let entry_id = get_u8(buf)?;
                if entry_id == TAG_END {
                    break;
                }
                let name = read_modified_utf8(buf)?;
                let tag = read_payload(buf, entry_id, depth + 1)?;
                entries.push((name, tag));
            }
            Nbt::Compound(entries)
        }
        TAG_INT_ARRAY => {
            let len = read_array_len(buf)?;
            let mut v = Vec::with_capacity(cap_hint(buf, len, 4));
            for _ in 0..len {
                v.push(get(buf, 4, Buf::get_i32)?);
            }
            Nbt::IntArray(v)
        }
        TAG_LONG_ARRAY => {
            let len = read_array_len(buf)?;
            let mut v = Vec::with_capacity(cap_hint(buf, len, 8));
            for _ in 0..len {
                v.push(get(buf, 8, Buf::get_i64)?);
            }
            Nbt::LongArray(v)
        }
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("unknown NBT tag id {other}"),
            ))
        }
    })
}

fn write_payload<B: BufMut>(buf: &mut B, tag: &Nbt) {
    match tag {
        Nbt::End => {}
        Nbt::Byte(v) => buf.put_i8(*v),
        Nbt::Short(v) => buf.put_i16(*v),
        Nbt::Int(v) => buf.put_i32(*v),
        Nbt::Long(v) => buf.put_i64(*v),
        Nbt::Float(v) => buf.put_f32(*v),
        Nbt::Double(v) => buf.put_f64(*v),
        Nbt::ByteArray(v) => {
            buf.put_i32(v.len() as i32);
            for b in v {
                buf.put_i8(*b);
            }
        }
        Nbt::String(s) => write_modified_utf8(buf, s),
        Nbt::List(items) => {
            // Element type is the first entry's id (lists are homogeneous);
            // an empty list is framed as TAG_End / length 0, like vanilla.
            let elem = items.first().map(Nbt::id).unwrap_or(TAG_END);
            // The wire format declares one element type for the whole list, so a
            // mixed-type `Vec` would serialize to a stream no decoder can read.
            // `Nbt::List(Vec<Nbt>)` can't enforce this at the type level; catch
            // the caller bug in dev rather than emit a corrupt list silently.
            debug_assert!(
                items.iter().all(|item| item.id() == elem),
                "heterogeneous NBT list: all elements must share a tag id"
            );
            buf.put_u8(elem);
            buf.put_i32(items.len() as i32);
            for item in items {
                write_payload(buf, item);
            }
        }
        Nbt::Compound(entries) => {
            for (name, tag) in entries {
                buf.put_u8(tag.id());
                write_modified_utf8(buf, name);
                write_payload(buf, tag);
            }
            buf.put_u8(TAG_END);
        }
        Nbt::IntArray(v) => {
            buf.put_i32(v.len() as i32);
            for i in v {
                buf.put_i32(*i);
            }
        }
        Nbt::LongArray(v) => {
            buf.put_i32(v.len() as i32);
            for l in v {
                buf.put_i64(*l);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Primitive helpers
// ---------------------------------------------------------------------------

fn get_u8<B: Buf>(buf: &mut B) -> Result<u8> {
    if !buf.has_remaining() {
        return Err(eof());
    }
    Ok(buf.get_u8())
}

fn get_i8<B: Buf>(buf: &mut B) -> Result<i8> {
    if !buf.has_remaining() {
        return Err(eof());
    }
    Ok(buf.get_i8())
}

/// Read a fixed-width big-endian value via one of `bytes`' `get_*` accessors,
/// checking that `n` bytes remain first.
fn get<B: Buf, T>(buf: &mut B, n: usize, f: fn(&mut B) -> T) -> Result<T> {
    if buf.remaining() < n {
        return Err(eof());
    }
    Ok(f(buf))
}

/// Array/list length: a signed big-endian `i32` that must be non-negative.
fn read_array_len<B: Buf>(buf: &mut B) -> Result<usize> {
    let len = get(buf, 4, Buf::get_i32)?;
    if len < 0 {
        return Err(Error::new(ErrorKind::InvalidData, "negative NBT length"));
    }
    Ok(len as usize)
}

/// A capacity hint for an element vector that never trusts `len` past what the
/// buffer could possibly hold. Each element occupies at least `elem_size` bytes,
/// so `remaining / elem_size` is a hard ceiling on the real count — clamping to
/// it stops a hostile length (up to `i32::MAX`) from reserving gigabytes before
/// the per-element reads get a chance to fail on truncation. We have no
/// `NbtAccounter`-style byte quota, so this is the allocation guard.
fn cap_hint<B: Buf>(buf: &B, len: usize, elem_size: usize) -> usize {
    len.min(buf.remaining() / elem_size)
}

// ---------------------------------------------------------------------------
// Modified UTF-8 (Java `DataInput::readUTF` / `DataOutput::writeUTF`)
// ---------------------------------------------------------------------------

/// Encode `s` as modified UTF-8 into a fresh buffer: `0x0000` and the
/// `0x0080..=0x07FF` range take two bytes, `0x0800..=0xFFFF` three, and
/// supplementary code points become a CESU-8 surrogate pair (two 3-byte units).
///
/// Stops before the encoded form would exceed `max_bytes`, always on a whole
/// `char` boundary so a supplementary code point is never split into a lone
/// surrogate. Java caps the wire form at 65535 bytes and *throws* past that;
/// our writers are infallible, so we truncate instead — strictly better than
/// silently wrapping the `u16` length prefix and desyncing the stream.
fn encode_modified_utf8(s: &str, max_bytes: usize) -> Vec<u8> {
    /// Encode one UTF-16 code unit as 1–3 bytes into `unit`, returning the length.
    fn encode_unit(unit: &mut [u8; 3], code: u32) -> usize {
        if (0x0001..=0x007F).contains(&code) {
            unit[0] = code as u8;
            1
        } else if code <= 0x07FF {
            unit[0] = 0xC0 | (code >> 6) as u8;
            unit[1] = 0x80 | (code & 0x3F) as u8;
            2
        } else {
            unit[0] = 0xE0 | (code >> 12) as u8;
            unit[1] = 0x80 | ((code >> 6) & 0x3F) as u8;
            unit[2] = 0x80 | (code & 0x3F) as u8;
            3
        }
    }

    let mut out = Vec::with_capacity(s.len().min(max_bytes));
    let mut unit = [0u8; 3];
    for c in s.chars() {
        let code = c as u32;
        // Encode this char's unit(s) into a scratch buffer first so we can check
        // the whole character fits before committing any of its bytes.
        let mut scratch = [0u8; 6];
        let n = if code <= 0xFFFF {
            let len = encode_unit(&mut unit, code);
            scratch[..len].copy_from_slice(&unit[..len]);
            len
        } else {
            let v = code - 0x1_0000;
            let hi = encode_unit(&mut unit, 0xD800 + (v >> 10));
            scratch[..hi].copy_from_slice(&unit[..hi]);
            let lo = encode_unit(&mut unit, 0xDC00 + (v & 0x3FF));
            scratch[hi..hi + lo].copy_from_slice(&unit[..lo]);
            hi + lo
        };
        if out.len() + n > max_bytes {
            break;
        }
        out.extend_from_slice(&scratch[..n]);
    }
    out
}

fn write_modified_utf8<B: BufMut>(buf: &mut B, s: &str) {
    // Java caps the encoded form at 65535 bytes. We truncate to that cap (on a
    // whole-char boundary) rather than let the byte length wrap the u16 prefix
    // and desync the stream — same behavior in debug and release.
    let bytes = encode_modified_utf8(s, u16::MAX as usize);
    buf.put_u16(bytes.len() as u16);
    buf.put_slice(&bytes);
}

fn read_modified_utf8<B: Buf>(buf: &mut B) -> Result<String> {
    let len = get(buf, 2, Buf::get_u16)? as usize;
    if buf.remaining() < len {
        return Err(eof());
    }
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    decode_modified_utf8(&bytes)
}

/// Decode modified UTF-8 to a `String`. Each 1/2/3-byte group yields one
/// UTF-16 code unit; surrogate pairs are recombined by `String::from_utf16`.
fn decode_modified_utf8(bytes: &[u8]) -> Result<String> {
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let a = bytes[i];
        if a & 0x80 == 0 {
            units.push(a as u16);
            i += 1;
        } else if a & 0xE0 == 0xC0 {
            let b = *bytes.get(i + 1).ok_or_else(malformed)?;
            if b & 0xC0 != 0x80 {
                return Err(malformed());
            }
            units.push((((a & 0x1F) as u16) << 6) | (b & 0x3F) as u16);
            i += 2;
        } else if a & 0xF0 == 0xE0 {
            let b = *bytes.get(i + 1).ok_or_else(malformed)?;
            let c = *bytes.get(i + 2).ok_or_else(malformed)?;
            if b & 0xC0 != 0x80 || c & 0xC0 != 0x80 {
                return Err(malformed());
            }
            units.push((((a & 0x0F) as u16) << 12) | (((b & 0x3F) as u16) << 6) | (c & 0x3F) as u16);
            i += 3;
        } else {
            return Err(malformed());
        }
    }
    String::from_utf16(&units).map_err(|_| malformed())
}

fn eof() -> Error {
    Error::new(ErrorKind::UnexpectedEof, "unexpected end of NBT buffer")
}

fn malformed() -> Error {
    Error::new(ErrorKind::InvalidData, "malformed modified UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    /// Round-trip a tag through the nameless (network) framing.
    fn round_network(tag: &Nbt) -> Nbt {
        let mut buf = BytesMut::new();
        write_network(&mut buf, tag);
        let mut slice = buf.freeze();
        let out = read_network(&mut slice).expect("decode");
        assert!(!slice.has_remaining(), "bytes left over after decode");
        out
    }

    /// Round-trip a tag through the named (disk) framing.
    fn round_named(name: &str, tag: &Nbt) -> (String, Nbt) {
        let mut buf = BytesMut::new();
        write_named(&mut buf, name, tag);
        let mut slice = buf.freeze();
        let out = read_named(&mut slice).expect("decode");
        assert!(!slice.has_remaining(), "bytes left over after decode");
        out
    }

    #[test]
    fn scalars_round_trip() {
        for tag in [
            Nbt::Byte(-7),
            Nbt::Byte(127),
            Nbt::Short(-30000),
            Nbt::Int(1_234_567),
            Nbt::Int(i32::MIN),
            Nbt::Long(-9_000_000_000),
            Nbt::Float(3.5),
            Nbt::Double(-1234.56789),
        ] {
            assert_eq!(round_network(&tag), tag);
        }
    }

    #[test]
    fn arrays_round_trip() {
        let b = Nbt::ByteArray(vec![-128, -1, 0, 1, 127]);
        assert_eq!(round_network(&b), b);

        let i = Nbt::IntArray(vec![i32::MIN, -1, 0, 1, i32::MAX]);
        assert_eq!(round_network(&i), i);

        let l = Nbt::LongArray(vec![i64::MIN, -1, 0, 1, i64::MAX]);
        assert_eq!(round_network(&l), l);

        // Empty arrays carry a zero length and nothing else.
        let e = Nbt::IntArray(vec![]);
        assert_eq!(round_network(&e), e);
    }

    #[test]
    fn strings_round_trip() {
        for s in [
            "",
            "hello world",
            "minecraft:overworld",
            // 2-byte (é), 3-byte (☃), and supplementary/4-byte (🌍) forms.
            "café ☃ 🌍",
            // Embedded NUL must survive the two-byte escape.
            "a\0b",
        ] {
            let tag = Nbt::String(s.to_string());
            assert_eq!(round_network(&tag), tag);
        }
    }

    #[test]
    fn empty_compound_round_trips() {
        let tag = Nbt::Compound(vec![]);
        assert_eq!(round_network(&tag), tag);
    }

    #[test]
    fn nested_compound_round_trips() {
        let tag = Nbt::Compound(vec![
            ("byte".into(), Nbt::Byte(1)),
            ("name".into(), Nbt::String("Vela".into())),
            (
                "pos".into(),
                Nbt::List(vec![Nbt::Double(1.0), Nbt::Double(2.0), Nbt::Double(3.0)]),
            ),
            (
                "inner".into(),
                Nbt::Compound(vec![
                    ("flag".into(), Nbt::Byte(0)),
                    ("ids".into(), Nbt::IntArray(vec![10, 20, 30])),
                ]),
            ),
            (
                "matrix".into(),
                Nbt::List(vec![
                    Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)]),
                    Nbt::List(vec![Nbt::Int(3), Nbt::Int(4)]),
                ]),
            ),
        ]);
        assert_eq!(round_network(&tag), tag);
    }

    #[test]
    fn empty_list_round_trips() {
        let tag = Nbt::List(vec![]);
        assert_eq!(round_network(&tag), tag);
    }

    #[test]
    fn named_root_round_trips() {
        let tag = Nbt::Compound(vec![("level".into(), Nbt::Int(42))]);
        let (name, decoded) = round_named("root", &tag);
        assert_eq!(name, "root");
        assert_eq!(decoded, tag);
    }

    #[test]
    fn named_and_nameless_framings_differ() {
        let tag = Nbt::Int(5);
        let mut named = BytesMut::new();
        write_named(&mut named, "x", &tag);
        let mut nameless = BytesMut::new();
        write_network(&mut nameless, &tag);
        // The named form carries the extra u16 length + "x" name bytes.
        assert_eq!(named.len(), nameless.len() + 3);
    }

    #[test]
    fn end_root_round_trips() {
        let mut buf = BytesMut::new();
        write_network(&mut buf, &Nbt::End);
        assert_eq!(&buf[..], &[TAG_END]);
        let mut slice = buf.freeze();
        assert_eq!(read_network(&mut slice).unwrap(), Nbt::End);
    }

    #[test]
    fn truncated_input_errors() {
        // Claims an int payload but provides no bytes for it.
        let mut slice = bytes::Bytes::from_static(&[TAG_INT]);
        assert!(read_network(&mut slice).is_err());
    }

    #[test]
    fn hostile_length_does_not_overallocate() {
        // A long-array root claiming i32::MAX elements (~16 GB) but carrying no
        // payload. The capacity hint must clamp to the empty buffer, so this
        // errors on the first element read instead of reserving gigabytes.
        let mut buf = BytesMut::new();
        buf.put_u8(TAG_LONG_ARRAY);
        buf.put_i32(i32::MAX);
        let mut slice = buf.freeze();
        assert!(read_network(&mut slice).is_err());
    }

    #[test]
    fn overlong_string_truncates_on_whole_chars() {
        // 30k of a 3-byte char = 90k bytes encoded, past the 65535 cap. The
        // writer must truncate on a char boundary (never a lone surrogate) and
        // emit a length that fits the u16 prefix, so the result still decodes.
        let tag = Nbt::String("☃".repeat(30_000));
        let mut buf = BytesMut::new();
        write_network(&mut buf, &tag);
        let mut slice = buf.freeze();
        let decoded = read_network(&mut slice).expect("decode truncated string");
        assert!(!slice.has_remaining());
        match decoded {
            Nbt::String(s) => {
                assert!(s.len() <= u16::MAX as usize);
                assert!(s.chars().all(|c| c == '☃'));
            }
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn constructors_and_accessors() {
        let c = Nbt::compound([
            ("text", Nbt::string("hi")),
            ("bold", Nbt::bool(true)),
        ]);
        assert_eq!(c.get("text").and_then(Nbt::as_str), Some("hi"));
        assert_eq!(c.get("bold"), Some(&Nbt::Byte(1)));
        assert_eq!(c.get("missing"), None);
        assert!(c.as_compound().is_some());
        assert_eq!(Nbt::Int(3).as_compound(), None);
        assert_eq!(Nbt::Int(3).get("x"), None);
        // The compound preserves insertion order.
        assert_eq!(
            c.as_compound().unwrap()[0].0,
            "text".to_string()
        );
    }

    #[test]
    fn known_byte_layout() {
        // A compound { "a": 5i8 } in nameless framing, byte-for-byte.
        let tag = Nbt::Compound(vec![("a".into(), Nbt::Byte(5))]);
        let mut buf = BytesMut::new();
        write_network(&mut buf, &tag);
        assert_eq!(
            &buf[..],
            &[
                TAG_COMPOUND, // root id
                TAG_BYTE,     // entry id
                0x00, 0x01,   // name length (1)
                b'a',         // name
                5,            // value
                TAG_END,      // compound terminator
            ]
        );
    }
}
