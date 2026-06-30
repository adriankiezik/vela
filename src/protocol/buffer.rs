//! Read/write helpers over an in-memory packet body, built on `bytes`.
//!
//! Mirrors the primitive accessors of the decompiled `FriendlyByteBuf`:
//! VarInt-prefixed UTF-8 strings, big-endian shorts/longs, and UUIDs.
//! `bytes::Buf`/`BufMut` handle cursor advancement; we add the
//! Minecraft-specific framing (VarInt, length-prefixed UTF-8).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use uuid::Uuid;

use super::varint::{get_varint, get_varlong, put_varint, put_varlong};

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

    #[allow(dead_code)] // counterpart to write_varlong; used by packet-layout tests.
    pub fn read_varlong(&mut self) -> std::io::Result<i64> {
        get_varlong(&mut self.buf)
    }

    pub fn read_u16(&mut self) -> std::io::Result<u16> {
        self.ensure(2)?;
        Ok(self.buf.get_u16())
    }

    pub fn read_i64(&mut self) -> std::io::Result<i64> {
        self.ensure(8)?;
        Ok(self.buf.get_i64())
    }

    /// Read a packed `BlockPos` long and unpack it to `(x, y, z)`. Mirrors
    /// `FriendlyByteBuf.readBlockPos` → `BlockPos.of(long)`: x and z occupy 26
    /// bits each and y the low 12, all sign-extended (`BlockPos.java`:
    /// PACKED_HORIZONTAL_LENGTH = 26, PACKED_Y_LENGTH = 12, X_OFFSET = 38,
    /// Z_OFFSET = 12, Y_OFFSET = 0).
    pub fn read_block_pos(&mut self) -> std::io::Result<(i32, i32, i32)> {
        Ok(unpack_block_pos(self.read_i64()?))
    }

    pub fn read_f32(&mut self) -> std::io::Result<f32> {
        self.ensure(4)?;
        Ok(self.buf.get_f32())
    }

    pub fn read_f64(&mut self) -> std::io::Result<f64> {
        self.ensure(8)?;
        Ok(self.buf.get_f64())
    }

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

    pub fn write_varlong(&mut self, v: i64) {
        put_varlong(&mut self.buf, v);
    }

    /// Write a `(x, y, z)` block position as a packed `BlockPos` long
    /// (`FriendlyByteBuf.writeBlockPos` → `BlockPos.asLong`).
    pub fn write_block_pos(&mut self, x: i32, y: i32, z: i32) {
        self.write_i64(pack_block_pos(x, y, z));
    }

    pub fn write_bool(&mut self, v: bool) {
        self.buf.put_u8(v as u8);
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.put_u8(v);
    }

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

// --- BlockPos packing (`BlockPos.asLong` / `BlockPos.of`) --------------------
// 26-bit x and z, 12-bit y; x at bit 38, z at bit 12, y at bit 0.
const BLOCKPOS_X_OFFSET: u32 = 38;
const BLOCKPOS_Z_OFFSET: u32 = 12;
const BLOCKPOS_HORIZONTAL_BITS: u32 = 26;
const BLOCKPOS_Y_BITS: u32 = 12;

/// Pack `(x, y, z)` into a `BlockPos` long, masking each field to its width.
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    let xm = (x as i64) & ((1 << BLOCKPOS_HORIZONTAL_BITS) - 1);
    let ym = (y as i64) & ((1 << BLOCKPOS_Y_BITS) - 1);
    let zm = (z as i64) & ((1 << BLOCKPOS_HORIZONTAL_BITS) - 1);
    (xm << BLOCKPOS_X_OFFSET) | (zm << BLOCKPOS_Z_OFFSET) | ym
}

/// Unpack a `BlockPos` long to `(x, y, z)`, sign-extending each field exactly as
/// vanilla's `BlockPos.getX/getY/getZ` do with paired shifts.
pub fn unpack_block_pos(node: i64) -> (i32, i32, i32) {
    // x: arithmetic shift right by 38 keeps the high 26 bits, sign-extended.
    let x = (node >> BLOCKPOS_X_OFFSET) as i32;
    // y: low 12 bits, sign-extended via shift left to the top then back down.
    let y = ((node << (64 - BLOCKPOS_Y_BITS)) >> (64 - BLOCKPOS_Y_BITS)) as i32;
    // z: bits [12,38), sign-extended.
    let z = ((node << (64 - BLOCKPOS_Z_OFFSET - BLOCKPOS_HORIZONTAL_BITS))
        >> (64 - BLOCKPOS_HORIZONTAL_BITS)) as i32;
    (x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_pos_round_trips_including_negatives() {
        for (x, y, z) in [
            (0, 0, 0),
            (1, 64, -1),
            (-30_000_000, -64, 30_000_000),
            (123_456, 319, -987_654),
            (-1, -2048, -3),
        ] {
            let packed = pack_block_pos(x, y, z);
            assert_eq!(unpack_block_pos(packed), (x, y, z), "({x},{y},{z})");
        }
    }

    #[test]
    fn block_pos_matches_known_layout() {
        // x=1,y=2,z=3 -> (1<<38) | (3<<12) | 2.
        assert_eq!(pack_block_pos(1, 2, 3), (1i64 << 38) | (3i64 << 12) | 2);
    }
}
