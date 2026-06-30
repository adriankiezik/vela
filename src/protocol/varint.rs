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
