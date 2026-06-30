//! A socket that can transparently switch to AES-CFB8 encryption mid-Login.
//!
//! Before the key exchange a connection is a plain `TcpStream`; once the client
//! sends `ServerboundKey` the server installs the stream cipher and every byte
//! thereafter is encrypted in both directions (`Connection.setEncryptionKey`).
//!
//! [`NetStream`] wraps a `TcpStream` and an optional cipher pair, implementing
//! `AsyncRead`/`AsyncWrite` so the framing code (`read_frame`/`send_packet`) is
//! oblivious to whether encryption is on. When the connection reaches Play it is
//! [`NetStream::into_split`] into independent read/write halves — each carrying
//! its own direction's cipher — for the reader and writer tasks.

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use super::crypto::Cfb8;

/// A connection's socket, optionally wrapped in the AES-CFB8 stream cipher.
pub enum NetStream {
    Plain(TcpStream),
    Encrypted(Box<EncStream>),
}

/// A `TcpStream` plus both directions' ciphers and the write side's spill
/// buffer. Reads decrypt newly-received bytes in place; writes encrypt the
/// caller's bytes into `pending` (advancing the cipher exactly once) and drain
/// it to the socket, so a partial socket write never re-encrypts.
pub struct EncStream {
    inner: TcpStream,
    decrypt: Cfb8,
    encrypt: Cfb8,
    pending: Vec<u8>,
    written: usize,
}

impl NetStream {
    pub fn plain(stream: TcpStream) -> Self {
        NetStream::Plain(stream)
    }

    /// Switch to encrypted framing using `secret` (the 16-byte AES shared key)
    /// as both the key and the initial IV for each direction. Consumes and
    /// returns the stream; an already-encrypted stream is returned unchanged
    /// (the key exchange runs at most once).
    pub fn enable_encryption(self, secret: &[u8; 16]) -> Self {
        match self {
            NetStream::Plain(inner) => NetStream::Encrypted(Box::new(EncStream {
                inner,
                decrypt: Cfb8::new(secret),
                encrypt: Cfb8::new(secret),
                pending: Vec::new(),
                written: 0,
            })),
            already => already,
        }
    }

    /// Split into independent read/write halves for the Play phase, each owning
    /// its direction's cipher.
    pub fn into_split(self) -> (NetReadHalf, NetWriteHalf) {
        match self {
            NetStream::Plain(stream) => {
                let (r, w) = stream.into_split();
                (NetReadHalf::Plain(r), NetWriteHalf::Plain(w))
            }
            NetStream::Encrypted(e) => {
                let EncStream {
                    inner,
                    decrypt,
                    encrypt,
                    pending,
                    written,
                } = *e;
                let (r, w) = inner.into_split();
                (
                    NetReadHalf::Encrypted(Box::new(EncReadHalf { inner: r, decrypt })),
                    NetWriteHalf::Encrypted(Box::new(EncWriteHalf {
                        inner: w,
                        encrypt,
                        pending,
                        written,
                    })),
                )
            }
        }
    }
}

impl AsyncRead for NetStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            NetStream::Encrypted(e) => poll_read_decrypt(&mut e.inner, &mut e.decrypt, cx, buf),
        }
    }
}

impl AsyncWrite for NetStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            NetStream::Encrypted(e) => {
                poll_write_encrypt(&mut e.inner, &mut e.encrypt, &mut e.pending, &mut e.written, cx, buf)
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_flush(cx),
            NetStream::Encrypted(e) => {
                ready!(drain_pending(&mut e.inner, &e.pending, &mut e.written, cx))?;
                e.pending.clear();
                e.written = 0;
                Pin::new(&mut e.inner).poll_flush(cx)
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            NetStream::Encrypted(e) => {
                ready!(drain_pending(&mut e.inner, &e.pending, &mut e.written, cx))?;
                Pin::new(&mut e.inner).poll_shutdown(cx)
            }
        }
    }
}

/// The read half of a (maybe-encrypted) connection in the Play phase.
pub enum NetReadHalf {
    Plain(OwnedReadHalf),
    Encrypted(Box<EncReadHalf>),
}

pub struct EncReadHalf {
    inner: OwnedReadHalf,
    decrypt: Cfb8,
}

impl AsyncRead for NetReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetReadHalf::Plain(s) => Pin::new(s).poll_read(cx, buf),
            NetReadHalf::Encrypted(e) => poll_read_decrypt(&mut e.inner, &mut e.decrypt, cx, buf),
        }
    }
}

/// The write half of a (maybe-encrypted) connection in the Play phase.
pub enum NetWriteHalf {
    Plain(OwnedWriteHalf),
    Encrypted(Box<EncWriteHalf>),
}

pub struct EncWriteHalf {
    inner: OwnedWriteHalf,
    encrypt: Cfb8,
    pending: Vec<u8>,
    written: usize,
}

impl AsyncWrite for NetWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            NetWriteHalf::Plain(s) => Pin::new(s).poll_write(cx, buf),
            NetWriteHalf::Encrypted(e) => {
                poll_write_encrypt(&mut e.inner, &mut e.encrypt, &mut e.pending, &mut e.written, cx, buf)
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetWriteHalf::Plain(s) => Pin::new(s).poll_flush(cx),
            NetWriteHalf::Encrypted(e) => {
                ready!(drain_pending(&mut e.inner, &e.pending, &mut e.written, cx))?;
                e.pending.clear();
                e.written = 0;
                Pin::new(&mut e.inner).poll_flush(cx)
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetWriteHalf::Plain(s) => Pin::new(s).poll_shutdown(cx),
            NetWriteHalf::Encrypted(e) => {
                ready!(drain_pending(&mut e.inner, &e.pending, &mut e.written, cx))?;
                Pin::new(&mut e.inner).poll_shutdown(cx)
            }
        }
    }
}

// --- Shared poll helpers, generic over the underlying half ------------------

/// Read into `buf` and decrypt only the freshly-received bytes in place. CFB8
/// processes one byte at a time in order, so a partial read decrypts correctly.
fn poll_read_decrypt<R: AsyncRead + Unpin>(
    inner: &mut R,
    decrypt: &mut Cfb8,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
) -> Poll<io::Result<()>> {
    let start = buf.filled().len();
    ready!(Pin::new(inner).poll_read(cx, buf))?;
    let filled = buf.filled_mut();
    decrypt.decrypt(&mut filled[start..]);
    Poll::Ready(Ok(()))
}

/// Encrypt `buf` into the spill buffer (advancing the cipher once) and drain as
/// much as the socket accepts. Any unwritten remainder stays buffered, so the
/// already-accepted plaintext is never encrypted twice on a partial write.
fn poll_write_encrypt<W: AsyncWrite + Unpin>(
    inner: &mut W,
    encrypt: &mut Cfb8,
    pending: &mut Vec<u8>,
    written: &mut usize,
    cx: &mut Context<'_>,
    buf: &[u8],
) -> Poll<io::Result<usize>> {
    // Flush any encrypted bytes left over from a prior accepted write first; only
    // once the buffer is empty may we take and encrypt new plaintext.
    if *written < pending.len() {
        ready!(drain_pending(inner, pending, written, cx))?;
    }
    pending.clear();
    *written = 0;
    if buf.is_empty() {
        return Poll::Ready(Ok(0));
    }
    pending.extend_from_slice(buf);
    encrypt.encrypt(pending);
    // Opportunistically push some out; a `Pending` here just leaves the rest
    // buffered for the next poll — `buf` is already fully accepted.
    while *written < pending.len() {
        match Pin::new(&mut *inner).poll_write(cx, &pending[*written..]) {
            Poll::Ready(Ok(0)) => break,
            Poll::Ready(Ok(n)) => *written += n,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => break,
        }
    }
    Poll::Ready(Ok(buf.len()))
}

/// Write out the buffered encrypted bytes from `written` to the end.
fn drain_pending<W: AsyncWrite + Unpin>(
    inner: &mut W,
    pending: &[u8],
    written: &mut usize,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    while *written < pending.len() {
        let n = ready!(Pin::new(&mut *inner).poll_write(cx, &pending[*written..]))?;
        if n == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write encrypted bytes",
            )));
        }
        *written += n;
    }
    Poll::Ready(Ok(()))
}
