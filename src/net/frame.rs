//! Async frame I/O over a socket: read one length-prefixed packet, or write one.
//!
//! Pure framing lives in `protocol::framing`; this adds the tokio read/write.
//! Pre-Play states write straight to the socket through `send_packet`; once in
//! Play, clientbound packets are framed by the sim and pumped by the write task,
//! so only `read_frame` is used there.
//!
//! Both functions take a `compression: Option<i32>` threshold. `None` means the
//! plain `VarInt(len)|VarInt(id)|body` framing; `Some(t)` switches to the
//! compressed layout (`VarInt(packetLength)|VarInt(dataLength)|data`) negotiated
//! mid-Login. The caller flips this from `None` to `Some` the instant the
//! `ClientboundLoginCompressionPacket` has been written.

use bytes::Bytes;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::debug;

use crate::protocol::buffer::PacketReader;
use crate::protocol::framing::{compress, decode_compressed, frame, MAX_FRAME_LEN};
use crate::protocol::varint::read_varint;

/// Read one frame and return the packet id and a reader over its body. Returns
/// `None` on a clean EOF (the length VarInt could not start).
///
/// Uncompressed (`compression == None`): `VarInt(length) | VarInt(id) | body`.
/// Compressed (`compression == Some(t)`): `VarInt(packetLength) |
/// VarInt(dataLength) | data`, where `data` is raw when `dataLength == 0` and
/// zlib-deflated otherwise; it is inflated to exactly `dataLength` bytes.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
    compression: Option<i32>,
) -> io::Result<Option<(i32, PacketReader)>> {
    let len = match read_varint(r).await {
        Ok(n) => n,
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    // Bound the length before allocating: a negative VarInt sign-extends to a
    // gigantic `usize`, and even a large positive one is an instant OOM from a
    // single unauthenticated packet. 2 MiB matches vanilla's MAX_PACKET_SIZE
    // (and `CompressionDecoder.MAXIMUM_COMPRESSED_LENGTH`).
    if !(0..=MAX_FRAME_LEN).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame length out of bounds",
        ));
    }
    let len = len as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    // `buf` is the bytes after `packetLength`. Uncompressed, it is already
    // `data`; compressed, it carries the `dataLength` prefix and (possibly
    // deflated) `data`, which `decode_compressed` unwraps — guarding both the
    // declared size and the inflated length.
    let data = match compression {
        None => Bytes::from(buf),
        Some(threshold) => decode_compressed(Bytes::from(buf), threshold)?,
    };
    let mut reader = PacketReader::new(data);
    let id = reader.read_varint()?;
    Ok(Some((id, reader)))
}

/// Frame and send a packet over an async writer, flushing immediately. Used by
/// the pre-Play states, which write to the socket synchronously with the
/// handshake handler rather than through the sim outbox. When `compression` is
/// `Some`, the framed bytes are re-wrapped through the compressor before going
/// out.
pub async fn send_packet<W: AsyncWrite + Unpin>(
    w: &mut W,
    id: i32,
    body: &[u8],
    compression: Option<i32>,
) -> io::Result<()> {
    let framed = frame(id, body);
    let out = match compression {
        None => framed,
        Some(threshold) => compress(&framed, threshold),
    };
    debug!(id = format!("{id:#04x}"), bytes = out.len(), "send");
    w.write_all(&out).await?;
    w.flush().await
}
