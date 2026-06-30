//! Packet framing: `VarInt(length) | VarInt(id) | body`.
//!
//! The length covers the id plus the body. Building a frame is pure and
//! synchronous, so both the async network layer (`net`) and the synchronous
//! simulation (`sim`, which produces clientbound packets inside the tick) share
//! this one helper rather than each reinventing the length prefix.

use bytes::{Bytes, BytesMut};

use super::varint::{put_varint, varint_len};

/// Upper bound on a single frame's declared length. Matches vanilla's
/// `MAX_PACKET_SIZE` (2 MiB); anything larger is treated as a protocol error
/// rather than allocated.
pub const MAX_FRAME_LEN: i32 = 2 * 1024 * 1024;

/// Frame a packet body into a ready-to-send buffer: `VarInt(len)|VarInt(id)|body`.
pub fn frame(id: i32, body: &[u8]) -> Bytes {
    let payload_len = varint_len(id) + body.len();
    let mut out = BytesMut::with_capacity(varint_len(payload_len as i32) + payload_len);
    put_varint(&mut out, payload_len as i32);
    put_varint(&mut out, id);
    out.extend_from_slice(body);
    out.freeze()
}
