//! Clientbound Play packet builders.
//!
//! Each returns a fully framed `Bytes` ready to drop into a player's outbox.
//! These run on the synchronous simulation thread, so unlike the pre-Play
//! senders in `net` they build a buffer rather than writing to a socket.
//!
//! Packet IDs are the registration-order indices from the decompiled 26.2
//! `GameProtocols` builder.

use bytes::Bytes;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;
use crate::protocol::nbt::{write_network, Nbt};

const CB_PLAY_GAME_EVENT: i32 = 38;
const CB_PLAY_KEEP_ALIVE: i32 = 44;
const CB_PLAY_LEVEL_CHUNK: i32 = 45;
const CB_PLAY_LOGIN: i32 = 49;
const CB_PLAY_PLAYER_POSITION: i32 = 72;
const CB_PLAY_SET_CHUNK_CACHE_CENTER: i32 = 94;
const CB_PLAY_SYSTEM_CHAT: i32 = 121;

/// `ClientboundGameEventPacket.LEVEL_CHUNKS_LOAD_START` — tells the client to
/// begin waiting for chunks; the "Loading terrain" screen clears once the
/// chunks around the player arrive.
pub const GAME_EVENT_LEVEL_CHUNKS_LOAD_START: u8 = 13;

/// The join parameters drawn from `server.properties`. The simulation builds one
/// from the loaded config and hands it to [`play_login`] and the chunk-streaming
/// loop, so the advertised view distance and the number of chunks we actually
/// send stay in lockstep.
#[derive(Clone, Copy)]
pub struct JoinParams {
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub hardcore: bool,
    pub online_mode: bool,
    /// Default game mode as a wire id (0=survival … 3=spectator).
    pub game_type: u8,
}

/// ClientboundLogin — the play "join game" packet. Spawns into a single
/// overworld dimension. `entity_id` is the player's own entity id, assigned by
/// the world; the rest of the join shape comes from `server.properties` via
/// [`JoinParams`].
pub fn play_login(entity_id: i32, p_in: &JoinParams) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_i32(entity_id); // entity id
    p.write_bool(p_in.hardcore); // hardcore
    p.write_varint(1); // levels: 1 dimension
    p.write_identifier("minecraft:overworld");
    p.write_varint(p_in.max_players); // max players
    p.write_varint(p_in.view_distance); // view distance
    p.write_varint(p_in.simulation_distance); // simulation distance
    p.write_bool(false); // reduced debug info
    p.write_bool(true); // show death screen
    p.write_bool(false); // limited crafting
    // CommonPlayerSpawnInfo:
    p.write_varint(1); // dimensionType holder: overworld is registry index 0 -> id+1
    p.write_identifier("minecraft:overworld"); // dimension
    p.write_i64(0); // seed (hashed, cosmetic)
    p.write_u8(p_in.game_type); // game type
    p.write_u8(0xFF); // previous game type = -1 (none)
    p.write_bool(false); // is debug
    p.write_bool(true); // is flat (renders a flat horizon/fog)
    p.write_bool(false); // last death location: absent
    p.write_varint(0); // portal cooldown
    p.write_varint(63); // sea level
    p.write_bool(p_in.online_mode); // online mode
    p.write_bool(false); // enforces secure chat
    frame(CB_PLAY_LOGIN, &p.buf)
}

pub fn game_event(event: u8, param: f32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_u8(event);
    p.write_f32(param);
    frame(CB_PLAY_GAME_EVENT, &p.buf)
}

pub fn set_chunk_center(x: i32, z: i32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(x);
    p.write_varint(z);
    frame(CB_PLAY_SET_CHUNK_CACHE_CENTER, &p.buf)
}

pub fn player_position(teleport_id: i32, x: f64, y: f64, z: f64) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(teleport_id);
    // PositionMoveRotation: position, delta movement, yaw, pitch.
    p.write_f64(x);
    p.write_f64(y);
    p.write_f64(z);
    p.write_f64(0.0); // dx
    p.write_f64(0.0); // dy
    p.write_f64(0.0); // dz
    p.write_f32(0.0); // yaw
    p.write_f32(0.0); // pitch
    p.write_i32(0); // relative flags: all absolute
    frame(CB_PLAY_PLAYER_POSITION, &p.buf)
}

pub fn keep_alive(id: i64) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_i64(id);
    frame(CB_PLAY_KEEP_ALIVE, &p.buf)
}

/// ClientboundSystemChatPacket: a text component (nameless network NBT) plus an
/// overlay flag. We render lines as a plain `{text:"…"}` compound and clear the
/// overlay so they land in the chat box rather than the action bar. System chat
/// is unsigned ("trusted"), so it sidesteps the whole secure-chat apparatus.
pub fn system_chat(text: &str) -> Bytes {
    let component = Nbt::Compound(vec![("text".to_string(), Nbt::String(text.to_string()))]);
    let mut p = PacketWriter::new();
    write_network(&mut p.buf, &component);
    p.write_bool(false); // overlay: false -> chat box (true would be the action bar)
    frame(CB_PLAY_SYSTEM_CHAT, &p.buf)
}

/// A flat chunk column: bedrock floor, dirt fill, grass surface at y=63, air
/// above. The block data and heightmaps come from `crate::world` (identical for
/// every column). Light is still sent empty — without a real light engine the
/// client falls back to full brightness, which is fine for a flat world.
pub fn flat_chunk(cx: i32, cz: i32) -> Bytes {
    let section_data = crate::world::flat_column_blob();
    let heightmaps = crate::world::flat_heightmaps();

    let mut p = PacketWriter::new();
    p.write_i32(cx);
    p.write_i32(cz);
    // --- ClientboundLevelChunkPacketData ---
    // Heightmaps: a map of type-id -> packed long[] (ByteBufCodecs.map + LONG_ARRAY).
    p.write_varint(heightmaps.len() as i32);
    for (type_id, longs) in heightmaps {
        p.write_varint(*type_id);
        p.write_varint(longs.len() as i32);
        for &l in longs {
            p.write_i64(l);
        }
    }
    p.write_varint(section_data.len() as i32); // section blob length
    p.write_bytes(section_data);
    p.write_varint(0); // block entities: none
    // --- ClientboundLightUpdatePacketData --- (four empty BitSets, two empty lists)
    p.write_varint(0); // sky-light mask
    p.write_varint(0); // block-light mask
    p.write_varint(0); // empty sky-light mask
    p.write_varint(0); // empty block-light mask
    p.write_varint(0); // sky-light arrays
    p.write_varint(0); // block-light arrays
    frame(CB_PLAY_LEVEL_CHUNK, &p.buf)
}
