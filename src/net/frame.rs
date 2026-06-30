//! Async frame I/O over a socket: read one length-prefixed packet, or write one.
//!
//! Pure framing lives in `protocol::framing`; this adds the tokio read/write.
//! Pre-Play states write straight to the socket through `send_packet`; once in
//! Play, clientbound packets are framed by the sim and pumped by the write task,
//! so only `read_frame` is used there.

use bytes::Bytes;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::debug;

use crate::protocol::buffer::PacketReader;
use crate::protocol::framing::{frame, MAX_FRAME_LEN};
use crate::protocol::varint::read_varint;

/// Read one frame: `VarInt(length) | VarInt(id) | body`. Returns `None` on a
/// clean EOF (the length VarInt could not start), otherwise the packet id and a
/// reader over the body.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
) -> io::Result<Option<(i32, PacketReader)>> {
    let len = match read_varint(r).await {
        Ok(n) => n,
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    // Bound the length before allocating: a negative VarInt sign-extends to a
    // gigantic `usize`, and even a large positive one is an instant OOM from a
    // single unauthenticated packet. 2 MiB matches vanilla's MAX_PACKET_SIZE.
    if !(0..=MAX_FRAME_LEN).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame length out of bounds",
        ));
    }
    let len = len as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    let mut reader = PacketReader::new(Bytes::from(buf));
    let id = reader.read_varint()?;
    Ok(Some((id, reader)))
}

/// Frame and send a packet over an async writer, flushing immediately. Used by
/// the pre-Play states, which write to the socket synchronously with the
/// handshake handler rather than through the sim outbox.
pub async fn send_packet<W: AsyncWrite + Unpin>(w: &mut W, id: i32, body: &[u8]) -> io::Result<()> {
    let out = frame(id, body);
    debug!(id = format!("{id:#04x}"), bytes = out.len(), "send");
    w.write_all(&out).await?;
    w.flush().await
}
