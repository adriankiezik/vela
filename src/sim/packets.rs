//! Clientbound Play packet builders.
//!
//! Each returns a fully framed `Bytes` ready to drop into a player's outbox.
//! These run on the synchronous simulation thread, so unlike the pre-Play
//! senders in `net` they build a buffer rather than writing to a socket.
//!
//! Packet IDs are the registration-order indices from the decompiled 26.2
//! `GameProtocols` builder.

use bytes::Bytes;
use uuid::Uuid;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;
use crate::protocol::nbt::{write_network, Nbt};

const CB_PLAY_ADD_ENTITY: i32 = 1;
const CB_PLAY_ENTITY_POSITION_SYNC: i32 = 35;
const CB_PLAY_GAME_EVENT: i32 = 38;
const CB_PLAY_KEEP_ALIVE: i32 = 44;
const CB_PLAY_LEVEL_CHUNK: i32 = 45;
const CB_PLAY_LOGIN: i32 = 49;
const CB_PLAY_MOVE_ENTITY_POS: i32 = 53;
const CB_PLAY_MOVE_ENTITY_POS_ROT: i32 = 54;
const CB_PLAY_MOVE_ENTITY_ROT: i32 = 56;
const CB_PLAY_PLAYER_INFO_REMOVE: i32 = 69;
const CB_PLAY_PLAYER_INFO_UPDATE: i32 = 70;
const CB_PLAY_PLAYER_POSITION: i32 = 72;
const CB_PLAY_REMOVE_ENTITIES: i32 = 77;
const CB_PLAY_ROTATE_HEAD: i32 = 83;
const CB_PLAY_SET_CHUNK_CACHE_CENTER: i32 = 94;
const CB_PLAY_SYSTEM_CHAT: i32 = 121;

/// `minecraft:player` network id — index 156 in the (alphabetical) entity-type
/// registration order of 26.2. Sent as the entity type in `AddEntity`.
const ENTITY_TYPE_PLAYER: i32 = 156;

/// `ClientboundPlayerInfoUpdatePacket` action set, encoded as the fixed 1-byte
/// bitset `writeEnumSet` produces over the 8 actions. Bits set: ADD_PLAYER (0),
/// UPDATE_GAME_MODE (2), UPDATE_LISTED (3), UPDATE_LATENCY (4) — the minimum for
/// a player to render (profile present) and show in the tab list.
const PLAYER_INFO_ACTIONS: u8 = 0b0001_1101;

/// `GameType.SURVIVAL` — the player drops onto the flat world's grass surface
/// and stands on it (client-side collision against the chunk data we stream).
const GAME_TYPE_SURVIVAL: u8 = 0;

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
/// overlay flag. The overlay is cleared so lines land in the chat box rather
/// than the action bar. System chat is unsigned ("trusted"), so it sidesteps
/// the whole secure-chat apparatus. Command replies and broadcasts both flow
/// through here, differing only in the component they carry.
pub fn system_chat_component(component: &Nbt) -> Bytes {
    let mut p = PacketWriter::new();
    write_network(&mut p.buf, component);
    p.write_bool(false); // overlay: false -> chat box (true would be the action bar)
    frame(CB_PLAY_SYSTEM_CHAT, &p.buf)
}

/// Convenience for a plain-text system message: `{text:"…"}`.
pub fn system_chat(text: &str) -> Bytes {
    system_chat_component(&super::text::text(text))
}

/// Pack a degree angle into a network byte, matching `Mth.packDegrees`:
/// `(byte) floor(angle * 256 / 360)`. The cast keeps the low 8 bits (signed),
/// so callers and the wire agree on the wrap.
pub fn pack_angle(deg: f32) -> i8 {
    (deg * 256.0 / 360.0).floor() as i32 as i8
}

/// Quantize a coordinate the way vanilla's `VecDeltaCodec` does:
/// `Math.round(d * 4096)` (i.e. `floor(d * 4096 + 0.5)`). Movement-packet deltas
/// are differences of these encoded values, so client and server agree on the
/// wrap and the deltas telescope exactly back to the absolute position.
pub fn enc(d: f64) -> i64 {
    (d * 4096.0 + 0.5).floor() as i64
}

/// One player-list entry for [`player_info_update`].
pub struct PlayerEntry {
    pub uuid: Uuid,
    pub name: String,
}

/// ClientboundPlayerInfoUpdatePacket — publish players to the tab list (and make
/// the client resolve their profile so their entity renders). We send a fixed
/// action subset (see [`PLAYER_INFO_ACTIONS`]); each entry is a UUID followed by
/// the per-action data in enum order. Offline mode → no profile properties, so
/// players show the default skin.
pub fn player_info_update(entries: &[PlayerEntry]) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_u8(PLAYER_INFO_ACTIONS); // fixed 1-byte EnumSet over the 8 actions
    p.write_varint(entries.len() as i32);
    for e in entries {
        p.write_uuid(e.uuid);
        // ADD_PLAYER: name + properties (offline: zero properties).
        p.write_utf(&e.name);
        p.write_varint(0);
        // UPDATE_GAME_MODE: survival.
        p.write_varint(GAME_TYPE_SURVIVAL as i32);
        // UPDATE_LISTED: visible in the tab list.
        p.write_bool(true);
        // UPDATE_LATENCY: unknown ping yet.
        p.write_varint(0);
    }
    frame(CB_PLAY_PLAYER_INFO_UPDATE, &p.buf)
}

/// ClientboundPlayerInfoRemovePacket — drop players from the tab list.
pub fn player_info_remove(uuids: &[Uuid]) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(uuids.len() as i32);
    for u in uuids {
        p.write_uuid(*u);
    }
    frame(CB_PLAY_PLAYER_INFO_REMOVE, &p.buf)
}

/// ClientboundAddEntityPacket — spawn an entity on a client. For a player the
/// type is `minecraft:player`, the movement is a zero `LpVec3` (a single 0
/// byte), and `data` is unused. Angles are already packed (`pack_angle`).
pub fn add_entity(
    entity_id: i32,
    uuid: Uuid,
    pos: (f64, f64, f64),
    yaw: i8,
    pitch: i8,
    head: i8,
) -> Bytes {
    let (x, y, z) = pos;
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_uuid(uuid);
    p.write_varint(ENTITY_TYPE_PLAYER);
    p.write_f64(x);
    p.write_f64(y);
    p.write_f64(z);
    p.write_u8(0); // movement: zero LpVec3 is a single 0 byte
    p.write_u8(pitch as u8); // xRot
    p.write_u8(yaw as u8); // yRot
    p.write_u8(head as u8); // yHeadRot
    p.write_varint(0); // data
    frame(CB_PLAY_ADD_ENTITY, &p.buf)
}

/// ClientboundRemoveEntitiesPacket — despawn entities by id.
pub fn remove_entities(entity_ids: &[i32]) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_ids.len() as i32);
    for id in entity_ids {
        p.write_varint(*id);
    }
    frame(CB_PLAY_REMOVE_ENTITIES, &p.buf)
}

/// ClientboundMoveEntityPacket.Pos — a position-only delta. Each component is
/// `round(cur * 4096) - round(base * 4096)` and must fit in a `short`.
pub fn move_entity_pos(entity_id: i32, dx: i16, dy: i16, dz: i16, on_ground: bool) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_i16(dx);
    p.write_i16(dy);
    p.write_i16(dz);
    p.write_bool(on_ground);
    frame(CB_PLAY_MOVE_ENTITY_POS, &p.buf)
}

/// ClientboundMoveEntityPacket.PosRot — a position delta plus packed rotation.
pub fn move_entity_pos_rot(
    entity_id: i32,
    dx: i16,
    dy: i16,
    dz: i16,
    yaw: i8,
    pitch: i8,
    on_ground: bool,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_i16(dx);
    p.write_i16(dy);
    p.write_i16(dz);
    p.write_u8(yaw as u8);
    p.write_u8(pitch as u8);
    p.write_bool(on_ground);
    frame(CB_PLAY_MOVE_ENTITY_POS_ROT, &p.buf)
}

/// ClientboundMoveEntityPacket.Rot — packed rotation only (no position change).
pub fn move_entity_rot(entity_id: i32, yaw: i8, pitch: i8, on_ground: bool) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_u8(yaw as u8);
    p.write_u8(pitch as u8);
    p.write_bool(on_ground);
    frame(CB_PLAY_MOVE_ENTITY_ROT, &p.buf)
}

/// ClientboundRotateHeadPacket — the head yaw, which is independent of the body
/// yaw carried by the move packets.
pub fn rotate_head(entity_id: i32, head: i8) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_u8(head as u8);
    frame(CB_PLAY_ROTATE_HEAD, &p.buf)
}

/// ClientboundEntityPositionSyncPacket — an absolute position/rotation resync,
/// used when a relative delta won't do (too large, on-ground flip, or the
/// periodic forced sync). Mirrors `PositionMoveRotation`: position, then delta
/// movement (zero — we don't model player velocity), then yaw/pitch.
pub fn entity_position_sync(
    entity_id: i32,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_f64(x);
    p.write_f64(y);
    p.write_f64(z);
    p.write_f64(0.0); // dx
    p.write_f64(0.0); // dy
    p.write_f64(0.0); // dz
    p.write_f32(yaw);
    p.write_f32(pitch);
    p.write_bool(on_ground);
    frame(CB_PLAY_ENTITY_POSITION_SYNC, &p.buf)
}

/// A generated chunk column: bedrock floor, stone fill, dirt + grass surface
/// following the noise heightmap, air above. The block data and heightmaps come
/// from `crate::world` and vary per chunk `(cx, cz)`. Light is still sent empty
/// — without a real light engine the client falls back to full brightness.
pub fn flat_chunk(cx: i32, cz: i32) -> Bytes {
    let section_data = crate::world::column_blob(cx, cz);
    let heightmaps = crate::world::heightmaps(cx, cz);

    let mut p = PacketWriter::new();
    p.write_i32(cx);
    p.write_i32(cz);
    // --- ClientboundLevelChunkPacketData ---
    // Heightmaps: a map of type-id -> packed long[] (ByteBufCodecs.map + LONG_ARRAY).
    p.write_varint(heightmaps.len() as i32);
    for (type_id, longs) in &heightmaps {
        p.write_varint(*type_id);
        p.write_varint(longs.len() as i32);
        for &l in longs {
            p.write_i64(l);
        }
    }
    p.write_varint(section_data.len() as i32); // section blob length
    p.write_bytes(&section_data);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;

    /// Strip the `len|id` frame header and return `(id, reader-at-body)`.
    fn unframe(bytes: Bytes) -> (i32, PacketReader) {
        let mut r = PacketReader::new(bytes);
        let _len = r.read_varint().unwrap();
        let id = r.read_varint().unwrap();
        (id, r)
    }

    #[test]
    fn pack_angle_matches_vanilla() {
        // (byte) floor(deg * 256 / 360), wrapping as a signed byte.
        assert_eq!(pack_angle(0.0), 0);
        assert_eq!(pack_angle(90.0), 64);
        assert_eq!(pack_angle(-90.0), -64);
        assert_eq!(pack_angle(180.0), -128); // 128 wraps to -128
        assert_eq!(pack_angle(360.0), 0); // wraps a full turn
    }

    #[test]
    fn enc_rounds_like_vec_delta_codec() {
        // Math.round(d * 4096) == floor(d * 4096 + 0.5).
        assert_eq!(enc(0.0), 0);
        assert_eq!(enc(1.0), 4096);
        assert_eq!(enc(-1.0), -4096);
        assert_eq!(enc(0.5), 2048);
        assert_eq!(enc(0.123), 504); // 503.808 -> 504
    }

    #[test]
    fn add_entity_layout() {
        let uuid = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let (id, mut r) = unframe(add_entity(7, uuid, (1.0, 64.0, -2.0), 64, -32, 64));
        assert_eq!(id, CB_PLAY_ADD_ENTITY);
        assert_eq!(r.read_varint().unwrap(), 7); // entity id
        assert_eq!(r.read_uuid().unwrap(), uuid);
        assert_eq!(r.read_varint().unwrap(), ENTITY_TYPE_PLAYER);
        assert_eq!(r.read_f64().unwrap(), 1.0);
        assert_eq!(r.read_f64().unwrap(), 64.0);
        assert_eq!(r.read_f64().unwrap(), -2.0);
        assert_eq!(r.read_u8().unwrap(), 0); // zero LpVec3 movement
        assert_eq!(r.read_u8().unwrap() as i8, -32); // xRot (pitch)
        assert_eq!(r.read_u8().unwrap() as i8, 64); // yRot (yaw)
        assert_eq!(r.read_u8().unwrap() as i8, 64); // yHeadRot
        assert_eq!(r.read_varint().unwrap(), 0); // data
    }

    #[test]
    fn move_entity_pos_layout() {
        let (id, mut r) = unframe(move_entity_pos(7, 16, -8, 0, true));
        assert_eq!(id, CB_PLAY_MOVE_ENTITY_POS);
        assert_eq!(r.read_varint().unwrap(), 7);
        assert_eq!(r.read_u16().unwrap() as i16, 16);
        assert_eq!(r.read_u16().unwrap() as i16, -8);
        assert_eq!(r.read_u16().unwrap() as i16, 0);
        assert!(r.read_bool().unwrap());
    }

    #[test]
    fn player_info_update_layout() {
        let uuid = Uuid::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888);
        let entries = [PlayerEntry {
            uuid,
            name: "Steve".to_string(),
        }];
        let (id, mut r) = unframe(player_info_update(&entries));
        assert_eq!(id, CB_PLAY_PLAYER_INFO_UPDATE);
        assert_eq!(r.read_u8().unwrap(), PLAYER_INFO_ACTIONS); // 1-byte action set
        assert_eq!(r.read_varint().unwrap(), 1); // entry count
        assert_eq!(r.read_uuid().unwrap(), uuid);
        assert_eq!(r.read_utf(16).unwrap(), "Steve"); // ADD_PLAYER name
        assert_eq!(r.read_varint().unwrap(), 0); // properties
        assert_eq!(r.read_varint().unwrap(), 0); // game mode = survival
        assert!(r.read_bool().unwrap()); // listed
        assert_eq!(r.read_varint().unwrap(), 0); // latency
    }

    #[test]
    fn remove_entities_layout() {
        let (id, mut r) = unframe(remove_entities(&[3, 5]));
        assert_eq!(id, CB_PLAY_REMOVE_ENTITIES);
        assert_eq!(r.read_varint().unwrap(), 2);
        assert_eq!(r.read_varint().unwrap(), 3);
        assert_eq!(r.read_varint().unwrap(), 5);
    }

    #[test]
    fn entity_position_sync_layout() {
        let (id, mut r) = unframe(entity_position_sync(7, 1.0, 64.0, -2.0, 90.0, -45.0, false));
        assert_eq!(id, CB_PLAY_ENTITY_POSITION_SYNC);
        assert_eq!(r.read_varint().unwrap(), 7);
        assert_eq!(r.read_f64().unwrap(), 1.0);
        assert_eq!(r.read_f64().unwrap(), 64.0);
        assert_eq!(r.read_f64().unwrap(), -2.0);
        assert_eq!(r.read_f64().unwrap(), 0.0); // dx
        assert_eq!(r.read_f64().unwrap(), 0.0); // dy
        assert_eq!(r.read_f64().unwrap(), 0.0); // dz
        assert_eq!(r.read_f32().unwrap(), 90.0);
        assert_eq!(r.read_f32().unwrap(), -45.0);
        assert!(!r.read_bool().unwrap());
    }
}
