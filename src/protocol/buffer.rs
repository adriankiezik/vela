//! Read/write helpers over an in-memory packet body, built on `bytes`.
//!
//! Mirrors the primitive accessors of the decompiled `FriendlyByteBuf`:
//! VarInt-prefixed UTF-8 strings, big-endian shorts/longs, and UUIDs.
//! `bytes::Buf`/`BufMut` handle cursor advancement; we add the
//! Minecraft-specific framing (VarInt, length-prefixed UTF-8).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use uuid::Uuid;

use super::varint::{get_varint, put_varint};

/// A cursor over a decoded packet body for reading fields in order.
pub struct PacketReader {
    buf: Bytes,
}

impl PacketReader {
    pub fn new(buf: Bytes) -> Self {
        Self { buf }
    }

    fn ensure(&self, n: usize) -> std::io::Result<()> {
        if self.buf.remaining() < n {
            return Err(eof("fixed-width field"));
        }
        Ok(())
    }

    pub fn read_varint(&mut self) -> std::io::Result<i32> {
        get_varint(&mut self.buf)
    }

    pub fn read_u16(&mut self) -> std::io::Result<u16> {
        self.ensure(2)?;
        Ok(self.buf.get_u16())
    }

    pub fn read_i64(&mut self) -> std::io::Result<i64> {
        self.ensure(8)?;
        Ok(self.buf.get_i64())
    }

    #[allow(dead_code)]
    pub fn read_u8(&mut self) -> std::io::Result<u8> {
        self.ensure(1)?;
        Ok(self.buf.get_u8())
    }

    #[allow(dead_code)]
    pub fn read_bool(&mut self) -> std::io::Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    /// VarInt length prefix followed by `len` UTF-8 bytes. `max` is the
    /// declared character cap from the source packet definition.
    pub fn read_utf(&mut self, max: usize) -> std::io::Result<String> {
        let len = self.read_varint()? as usize;
        if len > max * 4 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "string longer than declared maximum",
            ));
        }
        self.ensure(len)?;
        let slice = self.buf.copy_to_bytes(len);
        let s = String::from_utf8(slice.to_vec())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad utf8"))?;
        // The wire cap above bounds *bytes* (max*4); vanilla also rejects on the
        // decoded *character* count, so a multi-byte string can't slip past.
        if s.chars().count() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "string longer than declared maximum",
            ));
        }
        Ok(s)
    }

    pub fn read_uuid(&mut self) -> std::io::Result<Uuid> {
        self.ensure(16)?;
        Ok(Uuid::from_u128(self.buf.get_u128()))
    }
}

/// Accumulates a packet body. The caller frames it (id + length) on send.
#[derive(Default)]
pub struct PacketWriter {
    pub buf: BytesMut,
}

impl PacketWriter {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::new(),
        }
    }

    pub fn write_varint(&mut self, v: i32) {
        put_varint(&mut self.buf, v);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.buf.put_u8(v as u8);
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.put_u8(v);
    }

    #[allow(dead_code)]
    pub fn write_i16(&mut self, v: i16) {
        self.buf.put_i16(v);
    }

    pub fn write_i32(&mut self, v: i32) {
        self.buf.put_i32(v);
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.put_i64(v);
    }

    pub fn write_f32(&mut self, v: f32) {
        self.buf.put_f32(v);
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.put_f64(v);
    }

    pub fn write_utf(&mut self, s: &str) {
        self.write_varint(s.len() as i32);
        self.buf.put_slice(s.as_bytes());
    }

    /// A `namespace:path` resource location — wire-identical to a string.
    pub fn write_identifier(&mut self, id: &str) {
        self.write_utf(id);
    }

    /// Append raw bytes with no length prefix (the caller has already framed
    /// them, e.g. a pre-serialized chunk-section blob).
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.buf.put_slice(bytes);
    }

    #[allow(dead_code)]
    pub fn write_uuid(&mut self, v: Uuid) {
        self.buf.put_u128(v.as_u128());
    }
}

fn eof(what: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("unexpected end of buffer while reading {what}"),
    )
}
