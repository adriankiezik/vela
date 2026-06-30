//! VarInt codec — the foundation of the Minecraft wire format.
//!
//! Reference: decompiled `net.minecraft.network.FriendlyByteBuf` (MC 26.2).
//! VarInts are little-endian base-128 with a continuation bit (0x80).
//!
//! We keep this hand-written on purpose — it's the core of the protocol —
//! but operate over `bytes::Buf`/`BufMut` so it composes with the rest of
//! the codec instead of poking raw slice indices.

use bytes::{Buf, BufMut};
use tokio::io::{self, AsyncRead, AsyncReadExt};

const SEGMENT_BITS: u8 = 0x7F;
const CONTINUE_BIT: u8 = 0x80;

/// Read a VarInt directly from an async stream — used for the frame length
/// prefix, before we have the body buffered.
pub async fn read_varint<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<i32> {
    let mut value: i32 = 0;
    let mut position: u32 = 0;
    loop {
        let byte = r.read_u8().await?;
        value |= ((byte & SEGMENT_BITS) as i32) << position;
        if byte & CONTINUE_BIT == 0 {
            break;
        }
        position += 7;
        if position >= 32 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "VarInt too big"));
        }
    }
    Ok(value)
}

/// Read a VarInt from an in-memory buffer cursor.
pub fn get_varint<B: Buf>(buf: &mut B) -> io::Result<i32> {
    let mut value: i32 = 0;
    let mut position: u32 = 0;
    loop {
        if !buf.has_remaining() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VarInt ran past end of buffer",
            ));
        }
        let byte = buf.get_u8();
        value |= ((byte & SEGMENT_BITS) as i32) << position;
        if byte & CONTINUE_BIT == 0 {
            break;
        }
        position += 7;
        if position >= 32 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "VarInt too big"));
        }
    }
    Ok(value)
}

/// Encode a VarInt into a buffer.
pub fn put_varint<B: BufMut>(buf: &mut B, mut value: i32) {
    loop {
        let mut temp = (value as u32 & SEGMENT_BITS as u32) as u8;
        value = ((value as u32) >> 7) as i32;
        if value != 0 {
            temp |= CONTINUE_BIT;
        }
        buf.put_u8(temp);
        if value == 0 {
            break;
        }
    }
}

/// Number of bytes a VarInt would occupy on the wire.
pub fn varint_len(mut value: i32) -> usize {
    let mut n = 1;
    while (value as u32) & !(SEGMENT_BITS as u32) != 0 {
        value = ((value as u32) >> 7) as i32;
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------
// VarLong — the 64-bit sibling of VarInt, same continuation-bit scheme.
// Reference: decompiled `net.minecraft.network.VarLong` (MC 26.2). Used by
// `ClientboundSectionBlocksUpdatePacket`, whose `(stateId << 12) | localPos`
// entries can exceed 32 bits.
// ---------------------------------------------------------------------------

/// Read a VarLong from an in-memory buffer cursor (mirrors [`get_varint`]).
#[allow(dead_code)] // paired with put_varlong; exercised by the section-blocks tests.
pub fn get_varlong<B: Buf>(buf: &mut B) -> io::Result<i64> {
    let mut value: i64 = 0;
    let mut position: u32 = 0;
    loop {
        if !buf.has_remaining() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VarLong ran past end of buffer",
            ));
        }
        let byte = buf.get_u8();
        value |= ((byte & SEGMENT_BITS) as i64) << position;
        if byte & CONTINUE_BIT == 0 {
            break;
        }
        position += 7;
        if position >= 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VarLong too big",
            ));
        }
    }
    Ok(value)
}

/// Encode a VarLong into a buffer (mirrors [`put_varint`]; vanilla `VarLong.write`
/// shifts the unsigned value right 7 bits until the high bits clear).
pub fn put_varlong<B: BufMut>(buf: &mut B, mut value: i64) {
    loop {
        let mut temp = (value as u64 & SEGMENT_BITS as u64) as u8;
        value = ((value as u64) >> 7) as i64;
        if value != 0 {
            temp |= CONTINUE_BIT;
        }
        buf.put_u8(temp);
        if value == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    fn roundtrip(v: i64) {
        let mut b = BytesMut::new();
        put_varlong(&mut b, v);
        let mut cur = b.freeze();
        assert_eq!(get_varlong(&mut cur).unwrap(), v, "varlong roundtrip {v}");
    }

    #[test]
    fn varlong_roundtrips() {
        for v in [
            0i64,
            1,
            127,
            128,
            255,
            300,
            25565,
            i32::MAX as i64,
            i64::MAX,
            -1,
        ] {
            roundtrip(v);
        }
    }

    #[test]
    fn varlong_known_encoding() {
        // 300 -> 0xAC 0x02, matching the VarInt/VarLong worked example.
        let mut b = BytesMut::new();
        put_varlong(&mut b, 300);
        assert_eq!(&b[..], &[0xAC, 0x02]);
    }
}
