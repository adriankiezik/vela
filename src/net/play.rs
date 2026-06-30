//! The Play phase I/O: split the socket into a read task and a write task, and
//! bridge them to the simulation.
//!
//! - The **read task** decodes serverbound frames into `Serverbound` messages
//!   and forwards them to the sim. `read_frame` is not cancellation-safe (it
//!   consumes bytes incrementally), so it lives in its own task and is never
//!   raced against anything — only aborted wholesale on teardown.
//! - The **write task** drains this connection's outbox and pumps framed bytes
//!   to the socket, batching a burst into one flush.
//! - `play` registers the player, waits for either side to finish, then tears
//!   the other down and emits a single `Left`.

use tokio::io::{self, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

use crate::protocol::buffer::PacketReader;
use crate::sim::bridge::{Outbound, Serverbound, ToSim};

use super::frame::read_frame;

// Serverbound Play packet ids (registration order, decompiled `GameProtocols`).
const SB_PLAY_ACCEPT_TELEPORTATION: i32 = 0;
const SB_PLAY_CHAT_COMMAND: i32 = 7;
const SB_PLAY_CHAT_COMMAND_SIGNED: i32 = 8;
const SB_PLAY_CHAT: i32 = 9;
const SB_PLAY_KEEP_ALIVE: i32 = 28;
const SB_PLAY_MOVE_PLAYER_POS: i32 = 30;
const SB_PLAY_MOVE_PLAYER_POS_ROT: i32 = 31;
const SB_PLAY_MOVE_PLAYER_ROT: i32 = 32;
const SB_PLAY_MOVE_PLAYER_STATUS_ONLY: i32 = 33;
// Player-action ids, same registration-order source. `GameProtocols`
// SERVERBOUND_TEMPLATE: PlayerAbilities (line 101 → 40), PlayerCommand
// (line 103 → 42), Swing (line 124 → 63).
const SB_PLAY_PLAYER_ABILITIES: i32 = 40;
const SB_PLAY_PLAYER_COMMAND: i32 = 42;
const SB_PLAY_SWING: i32 = 63;
// Inventory ids: SetCarriedItem (53), SetCreativeModeSlot (56).
const SB_PLAY_SET_CARRIED_ITEM: i32 = 53;
const SB_PLAY_SET_CREATIVE_MODE_SLOT: i32 = 56;

/// `ServerboundMovePlayerPacket.FLAG_ON_GROUND` — bit 0 of the trailing flags
/// byte the movement packets carry (bit 1 is horizontal collision, ignored).
const MOVE_FLAG_ON_GROUND: u8 = 1;

/// Per-connection outbox depth. Sized to absorb the join sequence, which bursts
/// ~127 small packets (login + a `(2R+1)²` chunk square + teleport) in a single
/// tick before the write task has drained any. A future batched chunk streamer
/// would let this shrink.
const OUTBOX_CAP: usize = 1024;

/// Drive a connection through the Play phase. Returns when the player leaves.
pub async fn play(
    rd: OwnedReadHalf,
    wr: OwnedWriteHalf,
    peer: std::net::SocketAddr,
    uuid: Uuid,
    name: String,
    to_sim: mpsc::Sender<ToSim>,
) -> io::Result<()> {
    let (out_tx, out_rx) = mpsc::channel::<Outbound>(OUTBOX_CAP);

    // Register before spawning the reader so the sim observes `Joined` ahead of
    // any `Packet` for this player.
    if to_sim
        .send(ToSim::Joined {
            id: uuid,
            name: name.clone(),
            outbox: out_tx,
        })
        .await
        .is_err()
    {
        return Ok(()); // simulation is gone
    }

    let mut read = tokio::spawn(read_loop(rd, uuid, to_sim.clone()));
    let mut write = tokio::spawn(write_loop(wr, out_rx));

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
async fn read_loop(rd: OwnedReadHalf, uuid: Uuid, to_sim: mpsc::Sender<ToSim>) {
    // Buffered so the per-byte VarInt reads collapse into far fewer syscalls.
    let mut rd = BufReader::new(rd);
    while let Ok(Some((id, mut reader))) = read_frame(&mut rd).await {
        if let Some(packet) = decode_play(id, &mut reader) {
            if to_sim.send(ToSim::Packet { id: uuid, packet }).await.is_err() {
                break; // simulation is gone
            }
        }
    }
}

/// Pump framed bytes to the socket, batching a burst into one flush.
async fn write_loop(mut wr: OwnedWriteHalf, mut rx: mpsc::Receiver<Outbound>) -> io::Result<()> {
    while let Some(first) = rx.recv().await {
        match first {
            Outbound::Packet(b) => wr.write_all(&b).await?,
            Outbound::Close => break,
        }
        // Drain whatever else is already queued before flushing — this collapses
        // the join-sequence burst from ~127 flushes down to one.
        loop {
            match rx.try_recv() {
                Ok(Outbound::Packet(b)) => wr.write_all(&b).await?,
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

/// Decode a serverbound Play packet into a `Serverbound` the sim understands.
/// Unknown or malformed packets yield `None` and are simply dropped — each
/// frame is its own buffer, so unread trailing fields can't desync the stream.
fn decode_play(id: i32, reader: &mut PacketReader) -> Option<Serverbound> {
    match id {
        SB_PLAY_KEEP_ALIVE => Some(Serverbound::KeepAlive(reader.read_i64().ok()?)),
        SB_PLAY_ACCEPT_TELEPORTATION => {
            Some(Serverbound::AcceptTeleport(reader.read_varint().ok()?))
        }
        SB_PLAY_MOVE_PLAYER_POS => decode_move(reader, true, false),
        SB_PLAY_MOVE_PLAYER_POS_ROT => decode_move(reader, true, true),
        SB_PLAY_MOVE_PLAYER_ROT => decode_move(reader, false, true),
        SB_PLAY_MOVE_PLAYER_STATUS_ONLY => decode_move(reader, false, false),
        // ServerboundChatPacket: the message leads, then timestamp/salt/
        // signature/last-seen fields we ignore.
        SB_PLAY_CHAT => Some(Serverbound::Chat(reader.read_utf(256).ok()?)),
        // ServerboundChatCommand{,Signed}: the command string (no leading `/`)
        // leads both. The signed variant trails timestamp/salt/argument
        // signatures/last-seen, which we ignore — each frame is its own buffer,
        // so the unread tail can't desync the stream. The client sends the
        // unsigned form for commands without signable arguments (all of ours).
        SB_PLAY_CHAT_COMMAND | SB_PLAY_CHAT_COMMAND_SIGNED => {
            Some(Serverbound::ChatCommand(reader.read_utf(256).ok()?))
        }
        // ServerboundSwingPacket: a single `InteractionHand` written as its enum
        // ordinal via a VarInt (0 = main hand, 1 = off hand).
        SB_PLAY_SWING => Some(Serverbound::Swing {
            hand: reader.read_varint().ok()?,
        }),
        // ServerboundPlayerCommandPacket: entity id (the sender's own, dropped),
        // then the `Action` ordinal, then a VarInt data argument.
        SB_PLAY_PLAYER_COMMAND => {
            let _entity_id = reader.read_varint().ok()?;
            let action = reader.read_varint().ok()?;
            let _data = reader.read_varint().ok()?;
            Some(Serverbound::PlayerCommand { action })
        }
        // ServerboundPlayerAbilitiesPacket: a single flags byte (bit 0x02 = flying).
        SB_PLAY_PLAYER_ABILITIES => Some(Serverbound::PlayerAbilities {
            flags: reader.read_u8().ok()?,
        }),
        // ServerboundSetCarriedItemPacket: a single signed short hotbar slot.
        SB_PLAY_SET_CARRIED_ITEM => Some(Serverbound::SetCarriedItem {
            slot: reader.read_u16().ok()? as i16,
        }),
        // ServerboundSetCreativeModeSlotPacket: a signed short slot index then an
        // ItemStack (decoded here via the inventory codec; an unsupported data
        // component makes `read_item_stack` fail and the packet is dropped).
        SB_PLAY_SET_CREATIVE_MODE_SLOT => {
            let slot = reader.read_u16().ok()? as i16;
            let stack = crate::inventory::read_item_stack(reader).ok()?;
            Some(Serverbound::SetCreativeSlot { slot, stack })
        }
        _ => None,
    }
}

/// Decode a `ServerboundMovePlayerPacket` variant. The wire layout is the
/// present fields in order — position (3 doubles) when `has_pos`, rotation (yaw
/// then pitch, 2 floats) when `has_rot` — then a flags byte whose bit 0 is the
/// on-ground state. Absent fields are `None` so the sim keeps their prior value.
fn decode_move(reader: &mut PacketReader, has_pos: bool, has_rot: bool) -> Option<Serverbound> {
    let (mut x, mut y, mut z) = (None, None, None);
    if has_pos {
        x = Some(reader.read_f64().ok()?);
        y = Some(reader.read_f64().ok()?);
        z = Some(reader.read_f64().ok()?);
    }
    let (mut yaw, mut pitch) = (None, None);
    if has_rot {
        yaw = Some(reader.read_f32().ok()?);
        pitch = Some(reader.read_f32().ok()?);
    }
    let flags = reader.read_u8().ok()?;
    Some(Serverbound::Move {
        x,
        y,
        z,
        yaw,
        pitch,
        on_ground: flags & MOVE_FLAG_ON_GROUND != 0,
    })
}
