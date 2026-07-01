//! The Play phase I/O: split the socket into a read task and a write task, and
//! bridge them to the simulation.
//!
//! - The **read task** decodes serverbound frames into `Serverbound` messages
//!   (via `play_decode`) and forwards them to the sim. `read_frame` is not
//!   cancellation-safe (it consumes bytes incrementally), so it lives in its own
//!   task and is never raced against anything — only aborted wholesale on teardown.
//! - The **write task** drains this connection's outbox and pumps framed bytes
//!   to the socket, batching a burst into one flush.
//! - `play` registers the player, waits for either side to finish, then tears
//!   the other down and emits a single `Left`.

use tokio::io::{self, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

use crate::protocol::framing::Compressor;
use crate::sim::bridge::{Outbound, ToSim};

use super::frame::read_frame;
use super::play_decode::decode_play;
use super::stream::{NetReadHalf, NetWriteHalf};

/// Per-connection outbox depth. Sized to absorb the join sequence, which bursts
/// ~127 small packets (login + a `(2R+1)²` chunk square + teleport) in a single
/// tick before the write task has drained any. A future batched chunk streamer
/// would let this shrink.
const OUTBOX_CAP: usize = 1024;

/// Drive a connection through the Play phase. Returns when the player leaves.
///
/// `compression` is the threshold negotiated in Login (`None` if disabled). The
/// `sim` builds plain `frame()` bytes with no notion of compression; when a
/// threshold is set the write task re-wraps each outbound frame through
/// `framing::compress` before it hits the socket, and the read task inflates
/// inbound frames via `read_frame`. This keeps the compression transform wholly
/// inside `net`.
// Each argument is a distinct per-connection handle threaded from the login/config
// handshake into Play; bundling them into a struct would only add indirection.
#[allow(clippy::too_many_arguments)]
pub async fn play(
    rd: NetReadHalf,
    wr: NetWriteHalf,
    peer: std::net::SocketAddr,
    uuid: Uuid,
    name: String,
    to_sim: mpsc::Sender<ToSim>,
    compression: Option<i32>,
    view_distance: i32,
) -> io::Result<()> {
    let (out_tx, out_rx) = mpsc::channel::<Outbound>(OUTBOX_CAP);

    // Register before spawning the reader so the sim observes `Joined` ahead of
    // any `Packet` for this player.
    if to_sim
        .send(ToSim::Joined {
            id: uuid,
            name: name.clone(),
            outbox: out_tx,
            view_distance,
        })
        .await
        .is_err()
    {
        return Ok(()); // simulation is gone
    }

    let mut read = tokio::spawn(read_loop(rd, uuid, to_sim.clone(), compression));
    let mut write = tokio::spawn(write_loop(wr, out_rx, compression));

    // Whichever side finishes first, stop the other. The reader ends on client
    // disconnect or decode error; the writer ends on `Close`, a write error, or
    // the sim dropping the outbox.
    tokio::select! {
        _ = &mut read => write.abort(),
        _ = &mut write => read.abort(),
    }

    // Exactly one `Left`, here, regardless of which side ended things.
    let _ = to_sim.send(ToSim::Left { id: uuid }).await;
    info!(%peer, %name, "play ended");
    Ok(())
}

/// Decode frames and forward them to the sim until EOF or a decode error.
async fn read_loop(
    rd: NetReadHalf,
    uuid: Uuid,
    to_sim: mpsc::Sender<ToSim>,
    compression: Option<i32>,
) {
    // Buffered so the per-byte VarInt reads collapse into far fewer syscalls.
    let mut rd = BufReader::new(rd);
    while let Ok(Some((id, mut reader))) = read_frame(&mut rd, compression).await {
        if let Some(packet) = decode_play(id, &mut reader) {
            if to_sim.send(ToSim::Packet { id: uuid, packet }).await.is_err() {
                break; // simulation is gone
            }
        }
    }
}

/// Pump framed bytes to the socket, batching a burst into one flush. The sim
/// emits plain `frame()` bytes; once compression is active we re-wrap each frame
/// through `framing::compress` here, so the sim never deals with compression.
async fn write_loop(
    mut wr: NetWriteHalf,
    mut rx: mpsc::Receiver<Outbound>,
    compression: Option<i32>,
) -> io::Result<()> {
    // Apply the compressed framing to a sim-built frame iff a threshold is set,
    // reusing one `Deflater`/scratch buffer for the whole connection rather than
    // allocating per packet (review follow-up F2).
    let mut compressor = Compressor::new();
    let mut wrap = |b: bytes::Bytes| match compression {
        Some(threshold) => compressor.compress_frame(&b, threshold),
        None => b,
    };
    while let Some(first) = rx.recv().await {
        match first {
            Outbound::Packet(b) => wr.write_all(&wrap(b)).await?,
            Outbound::Close => break,
        }
        // Drain whatever else is already queued before flushing — this collapses
        // the join-sequence burst from ~127 flushes down to one.
        loop {
            match rx.try_recv() {
                Ok(Outbound::Packet(b)) => wr.write_all(&wrap(b)).await?,
                Ok(Outbound::Close) => {
                    wr.flush().await?;
                    return Ok(());
                }
                Err(_) => break,
            }
        }
        wr.flush().await?;
    }
    Ok(())
}
