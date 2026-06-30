//! Packet framing: `VarInt(length) | VarInt(id) | body`.
//!
//! The length covers the id plus the body. Building a frame is pure and
//! synchronous, so both the async network layer (`net`) and the synchronous
//! simulation (`sim`, which produces clientbound packets inside the tick) share
//! this one helper rather than each reinventing the length prefix.
//!
//! # Compression
//!
//! Minecraft enables zlib compression mid-Login (see `connection.rs`). Once the
//! `ClientboundLoginCompressionPacket` is sent, every subsequent frame uses the
//! compressed layout:
//!
//! ```text
//! VarInt(packetLength) | VarInt(dataLength) | (zlib) data
//! ```
//!
//! where `data = VarInt(id)|body`, `packetLength` is the byte length of
//! everything after it (the `dataLength` VarInt plus the trailing bytes), and:
//!
//! - `dataLength == 0` → `data` follows UNCOMPRESSED (used when `data` is below
//!   the negotiated threshold, so deflating it would only add overhead).
//! - `dataLength > 0`  → `dataLength` is the *uncompressed* size and the trailing
//!   bytes are the zlib-deflated `data`.
//!
//! Reference: decompiled `CompressionEncoder.encode` / `CompressionDecoder.decode`
//! (MC 26.2). The decoder rejects a compressed packet whose declared size is
//! below the threshold ("Badly compressed packet") or above the protocol maximum.
//!
//! The `sim` stays compression-agnostic: it always builds plain [`frame`] bytes.
//! `net` re-wraps those through [`compress`] on the way out once compression is
//! active, and `read_frame` unwraps inbound frames via [`decode_compressed`].

use std::io::{self, Read, Write};

use bytes::{Bytes, BytesMut};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use super::varint::{get_varint, put_varint, varint_len};

/// Upper bound on a single frame's declared length. Matches vanilla's
/// `MAX_PACKET_SIZE` / `CompressionDecoder.MAXIMUM_COMPRESSED_LENGTH` (2 MiB);
/// anything larger is treated as a protocol error rather than allocated.
pub const MAX_FRAME_LEN: i32 = 2 * 1024 * 1024;

/// Upper bound on a compressed packet's *decompressed* size. Matches vanilla's
/// `CompressionDecoder.MAXIMUM_UNCOMPRESSED_LENGTH` (8 MiB); a larger declared
/// `dataLength` is rejected before inflating, so a tiny packet can't claim a
/// huge output and force a huge allocation.
pub const MAX_UNCOMPRESSED_LEN: i32 = 8 * 1024 * 1024;

/// Frame a packet body into a ready-to-send buffer: `VarInt(len)|VarInt(id)|body`.
pub fn frame(id: i32, body: &[u8]) -> Bytes {
    let payload_len = varint_len(id) + body.len();
    let mut out = BytesMut::with_capacity(varint_len(payload_len as i32) + payload_len);
    put_varint(&mut out, payload_len as i32);
    put_varint(&mut out, id);
    out.extend_from_slice(body);
    out.freeze()
}

/// Re-wrap an already-built uncompressed frame (`VarInt(len)|data`, as produced
/// by [`frame`]) into the compressed layout. This is how `net` applies
/// compression to bytes the `sim` framed without knowing compression exists: the
/// leading length prefix is stripped to recover `data = VarInt(id)|body`, then
/// `data` is re-encoded with [`encode_compressed`].
pub fn compress(uncompressed_frame: &[u8], threshold: i32) -> Bytes {
    let mut cursor: &[u8] = uncompressed_frame;
    // Discard the uncompressed length prefix; the remainder is exactly `data`.
    // `frame` always writes a valid leading VarInt, so this never fails.
    get_varint(&mut cursor).expect("frame() always prefixes a valid VarInt length");
    encode_compressed(cursor, threshold)
}

/// Encode `data` (`VarInt(id)|body`) into a compressed frame. Mirrors
/// `CompressionEncoder.encode`: below the threshold the data is stored verbatim
/// behind a `dataLength` of 0; at or above it the data is zlib-deflated and the
/// real uncompressed length is written.
pub fn encode_compressed(data: &[u8], threshold: i32) -> Bytes {
    // Vanilla `CompressionEncoder.encode` throws "Packet too big" when the
    // uncompressed data exceeds 8 MiB. We are the producer of these bytes, so a
    // debug assert documents and guards the invariant without burdening the
    // release path with a fallible return on every outbound packet.
    debug_assert!(
        data.len() <= MAX_UNCOMPRESSED_LEN as usize,
        "Packet too big (is {}, should be less than {MAX_UNCOMPRESSED_LEN})",
        data.len()
    );
    let mut payload = BytesMut::new();
    if (data.len() as i32) < threshold {
        put_varint(&mut payload, 0);
        payload.extend_from_slice(data);
    } else {
        put_varint(&mut payload, data.len() as i32);
        payload.extend_from_slice(&zlib_compress(data));
    }
    // `packetLength` = length of everything after it (dataLength VarInt + bytes).
    let mut out = BytesMut::with_capacity(varint_len(payload.len() as i32) + payload.len());
    put_varint(&mut out, payload.len() as i32);
    out.extend_from_slice(&payload);
    out.freeze()
}

/// Decode one compressed frame's payload (the `packetLength` bytes: a
/// `dataLength` VarInt followed by either raw or deflated `data`) back into
/// `data = VarInt(id)|body`. Mirrors `CompressionDecoder.decode`, including its
/// guards that a *compressed* packet's declared size is neither below the
/// threshold ("Badly compressed packet") nor above the protocol maximum.
pub fn decode_compressed(mut payload: Bytes, threshold: i32) -> io::Result<Bytes> {
    let data_len = get_varint(&mut payload)?;
    if data_len == 0 {
        // Stored uncompressed: the rest of the payload is `data` verbatim.
        return Ok(payload);
    }
    if data_len < threshold {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Badly compressed packet - size of {data_len} is below server threshold of {threshold}"
            ),
        ));
    }
    if data_len > MAX_UNCOMPRESSED_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Badly compressed packet - size of {data_len} is larger than protocol maximum of {MAX_UNCOMPRESSED_LEN}"
            ),
        ));
    }
    zlib_decompress(&payload, data_len as usize)
}

/// zlib-deflate `data`. Default level (6) matches Java's `Deflater` default.
fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(
        Vec::with_capacity(data.len() / 2 + 16),
        Compression::default(),
    );
    enc.write_all(data).expect("zlib deflate into Vec is infallible");
    enc.finish().expect("zlib finish into Vec is infallible")
}

/// zlib-inflate `data` to exactly `expected` bytes, erroring if the stream
/// decompresses to a different length than declared (vanilla's actual-vs-declared
/// check in `CompressionDecoder.inflate`).
fn zlib_decompress(data: &[u8], expected: usize) -> io::Result<Bytes> {
    let mut dec = ZlibDecoder::new(data);
    let mut out = vec![0u8; expected];
    dec.read_exact(&mut out).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Badly compressed packet - decompressed payload shorter than declared size",
        )
    })?;
    // Nothing should remain: a longer output means the declared size lied.
    if dec.read(&mut [0u8; 1])? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Badly compressed packet - decompressed payload longer than declared size",
        ));
    }
    Ok(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: i32 = 256;

    /// Parse a compressed wire frame: assert `packetLength` covers the rest, then
    /// return `(declared dataLength, payload-after-packetLength)`.
    fn split(wire: &Bytes) -> (i32, Bytes) {
        let mut cur = wire.clone();
        let packet_len = get_varint(&mut cur).unwrap() as usize;
        assert_eq!(packet_len, cur.len(), "packetLength must cover the remainder");
        let mut peek = cur.clone();
        let declared = get_varint(&mut peek).unwrap();
        (declared, cur)
    }

    /// Read `data = VarInt(id)|body` and assert it matches.
    fn assert_data(data: Bytes, id: i32, body: &[u8]) {
        let mut cur = data;
        assert_eq!(get_varint(&mut cur).unwrap(), id);
        assert_eq!(&cur[..], body);
    }

    /// Round-trip a frame whose `data` is at or above the threshold: it must be
    /// stored compressed (declared length == uncompressed length) and decode back
    /// to the original id and body.
    #[test]
    fn round_trip_above_threshold_compresses() {
        let body = vec![0xABu8; 1000]; // > THRESHOLD, and highly compressible
        let wire = compress(&frame(0x42, &body), THRESHOLD);

        // The compressed wire form should be smaller than the raw data.
        assert!(wire.len() < body.len());

        let (declared, payload) = split(&wire);
        assert_eq!(declared, (varint_len(0x42) + body.len()) as i32);

        let data = decode_compressed(payload, THRESHOLD).unwrap();
        assert_data(data, 0x42, &body);
    }

    /// A frame below the threshold is stored verbatim with `dataLength == 0`.
    #[test]
    fn under_threshold_passes_through_uncompressed() {
        let body = vec![0x07u8; 10]; // < THRESHOLD
        let wire = compress(&frame(0x05, &body), THRESHOLD);

        let (declared, payload) = split(&wire);
        assert_eq!(declared, 0, "dataLength must be 0");

        let data = decode_compressed(payload, THRESHOLD).unwrap();
        assert_data(data, 0x05, &body);
    }

    /// The decoder rejects a *compressed* packet whose declared `dataLength` is
    /// below the threshold — vanilla's "Badly compressed packet" guard.
    #[test]
    fn decode_rejects_under_threshold_declared_size() {
        // Hand-build a payload: dataLength = 5 (< THRESHOLD) + some deflated bytes.
        let mut payload = BytesMut::new();
        put_varint(&mut payload, 5);
        payload.extend_from_slice(&zlib_compress(&[1, 2, 3, 4, 5]));
        let err = decode_compressed(payload.freeze(), THRESHOLD).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("below server threshold"));
    }

    /// The decoder rejects a declared size beyond the protocol maximum without
    /// attempting to allocate/inflate it.
    #[test]
    fn decode_rejects_oversized_declared_length() {
        let mut payload = BytesMut::new();
        put_varint(&mut payload, MAX_UNCOMPRESSED_LEN + 1);
        payload.extend_from_slice(&[0x78, 0x9c]); // arbitrary
        let err = decode_compressed(payload.freeze(), THRESHOLD).unwrap_err();
        assert!(err.to_string().contains("protocol maximum"));
    }
}
