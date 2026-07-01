//! Clientbound Play packet builders.
//!
//! Each returns a fully framed `Bytes` ready to drop into a player's outbox.
//! These run on the synchronous simulation thread, so unlike the pre-Play
//! senders in `net` they build a buffer rather than writing to a socket.
//!
//! Packet IDs are the registration-order indices from the decompiled 26.2
//! `GameProtocols` builder.
//!
//! This module holds the general play builders (join, movement, entities, chat,
//! metadata). Self-contained domains keep their own builders next to their data
//! model instead — see `crate::inventory` for the inventory/container packets.

use bytes::Bytes;
use uuid::Uuid;

use crate::ids::BlockState;
use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;
use crate::protocol::nbt::{write_network, Nbt};

const CB_PLAY_ADD_ENTITY: i32 = 1;
const CB_PLAY_ANIMATE: i32 = 2;
// Block-edit feedback packets (clientbound registration order in
// `GameProtocols.CLIENTBOUND_TEMPLATE`, bundle delimiter = index 0):
// BLOCK_CHANGED_ACK (line 137 → 4), BLOCK_UPDATE (line 141 → 8),
// SECTION_BLOCKS_UPDATE (line 217 → 84).
const CB_PLAY_BLOCK_CHANGED_ACK: i32 = 4;
const CB_PLAY_BLOCK_UPDATE: i32 = 8;
const CB_PLAY_ENTITY_POSITION_SYNC: i32 = 35;
const CB_PLAY_FORGET_LEVEL_CHUNK: i32 = 37;
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
const CB_PLAY_SECTION_BLOCKS_UPDATE: i32 = 84;
const CB_PLAY_SET_ENTITY_DATA: i32 = 99;
const CB_PLAY_SET_TIME: i32 = 113;
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

// Weather `ClientboundGameEventPacket.Type` ids (from `ClientboundGameEventPacket`):
// START_RAINING=1, STOP_RAINING=2, RAIN_LEVEL_CHANGE=7, THUNDER_LEVEL_CHANGE=8.
// START/STOP carry a 0.0 param; the LEVEL_CHANGE events carry the new level.
/// `ClientboundGameEventPacket.START_RAINING` — sky begins to rain (param 0).
pub const GAME_EVENT_START_RAINING: u8 = 1;
/// `ClientboundGameEventPacket.STOP_RAINING` — sky clears (param 0).
pub const GAME_EVENT_STOP_RAINING: u8 = 2;
/// `ClientboundGameEventPacket.RAIN_LEVEL_CHANGE` — param is the new rain level 0..=1.
pub const GAME_EVENT_RAIN_LEVEL_CHANGE: u8 = 7;
/// `ClientboundGameEventPacket.THUNDER_LEVEL_CHANGE` — param is the new thunder level 0..=1.
pub const GAME_EVENT_THUNDER_LEVEL_CHANGE: u8 = 8;

/// `WORLD_CLOCK` registry id of `minecraft:overworld` — the first entry registered
/// in `WorldClocks.bootstrap` (overworld=0, the_end=1), so its `holderRegistry`
/// wire id is 0. Sent as the clock holder key in [`set_time`].
pub const WORLD_CLOCK_OVERWORLD: i32 = 0;

/// One world-clock update inside a [`set_time`] packet, mirroring vanilla
/// `ClockNetworkState(totalTicks, partialTick, rate)` keyed by a `Holder<WorldClock>`.
/// `rate` of 0.0 tells the client the clock is paused (frozen daylight) — in 26.2
/// the frozen state is signalled by a zero rate, **not** by a negative day time as
/// in pre-1.21.5 clients.
#[derive(Clone, Copy)]
pub struct ClockUpdate {
    /// `WORLD_CLOCK` registry id (e.g. [`WORLD_CLOCK_OVERWORLD`]).
    pub clock_id: i32,
    /// The clock's monotonically-advancing tick counter (the "day time"). The
    /// client takes `total_ticks % 24000` for the time of day.
    pub total_ticks: i64,
    /// Sub-tick fraction accumulated toward the next whole tick.
    pub partial_tick: f32,
    /// Ticks advanced per server tick; 0.0 = paused/frozen.
    pub rate: f32,
}

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
    p.write_bool(false); // is flat: false now that terrain is noise-generated (normal horizon/fog)
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

/// ClientboundSetTimePacket (26.2). The clock model was reworked in this version:
/// the old `gameTime: long`, `dayTime: long`, `tickDayTime: bool` triple became
/// `gameTime: long` plus a `Map<Holder<WorldClock>, ClockNetworkState>` of per-clock
/// updates. Wire layout:
///   * `gameTime`: i64 (fixed big-endian `LONG`) — the overworld world age.
///   * `clockUpdates`: a `ByteBufCodecs.map` — VarInt count, then per entry:
///       - clock holder: VarInt registry id (`holderRegistry`, overworld=0),
///       - `totalTicks`: VarLong,
///       - `partialTick`: f32,
///       - `rate`: f32 (0.0 ⇒ paused / frozen daylight).
///
/// Vanilla's periodic 1 s resync (`forceGameTimeSynchronization`) sends an *empty*
/// map (gameTime only); the clocks are resent in full on join and whenever the
/// clock changes (rate flip, set-time command). Pass an empty `clocks` slice for
/// the gameTime-only periodic sync.
pub fn set_time(game_time: i64, clocks: &[ClockUpdate]) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_i64(game_time); // ByteBufCodecs.LONG
    p.write_varint(clocks.len() as i32); // map size
    for c in clocks {
        p.write_varint(c.clock_id); // Holder<WorldClock>: registry id
        p.write_varlong(c.total_ticks); // ClockNetworkState.totalTicks (VAR_LONG)
        p.write_f32(c.partial_tick); // ClockNetworkState.partialTick
        p.write_f32(c.rate); // ClockNetworkState.rate
    }
    frame(CB_PLAY_SET_TIME, &p.buf)
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

/// `ClientboundAnimatePacket.SWING_MAIN_HAND` — the main-arm swing action byte.
pub const ANIMATE_SWING_MAIN_HAND: u8 = 0;
/// `ClientboundAnimatePacket.SWING_OFF_HAND` — the off-hand swing action byte.
pub const ANIMATE_SWING_OFF_HAND: u8 = 3;

/// ClientboundAnimatePacket — play a one-shot animation on an entity for every
/// viewer. `entity_id` is a VarInt; `action` is an unsigned byte (vanilla
/// `readUnsignedByte`): 0 = swing main hand, 3 = swing off hand, 2 = wake up,
/// 4 = critical hit, 5 = magic critical hit.
pub fn animate(entity_id: i32, action: u8) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_u8(action);
    frame(CB_PLAY_ANIMATE, &p.buf)
}

// --- Entity metadata (`ClientboundSetEntityDataPacket`) wire constants ---
// Accessor ids from `Entity.defineId` order: DATA_SHARED_FLAGS_ID = 0 (Byte),
// DATA_POSE = 6 (Pose).
const DATA_SHARED_FLAGS_ID: u8 = 0;
const DATA_POSE: u8 = 6;
// `EntityDataSerializers` registration order gives the serializer type ids:
// BYTE = 0 (first registered), POSE = 20.
const ENTITY_DATA_SERIALIZER_BYTE: i32 = 0;
const ENTITY_DATA_SERIALIZER_POSE: i32 = 20;
// Shared-flags bit positions (vanilla `1 << FLAG_*`): SHIFT_KEY_DOWN = bit 1,
// SPRINTING = bit 3.
const SHARED_FLAG_CROUCHING: u8 = 1 << 1; // 0x02
const SHARED_FLAG_SPRINTING: u8 = 1 << 3; // 0x08
// `Pose` ordinals (== id()): STANDING = 0, CROUCHING = 5.
const POSE_STANDING: i32 = 0;
const POSE_CROUCHING: i32 = 5;
/// `ClientboundSetEntityDataPacket.EOF_MARKER` — the index byte that terminates
/// the packed metadata list.
const ENTITY_DATA_EOF: u8 = 0xFF;

/// ClientboundSetEntityDataPacket — sync a player's action metadata to viewers.
/// Layout: `entity_id` (VarInt), then packed `DataValue`s — each `index` (u8) +
/// `serializer_type_id` (VarInt) + value — terminated by the index `0xFF`.
///
/// We emit two entries: the shared-flags byte (index 0, `Byte` serializer) with
/// bit 0x02 = crouching and bit 0x08 = sprinting, and the pose (index 6, `Pose`
/// serializer, a VarInt id) set to CROUCHING while sneaking else STANDING.
pub fn set_entity_data(entity_id: i32, sneaking: bool, sprinting: bool) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);

    // DATA_SHARED_FLAGS_ID (Byte): crouching + sprinting bits.
    let mut flags = 0u8;
    if sneaking {
        flags |= SHARED_FLAG_CROUCHING;
    }
    if sprinting {
        flags |= SHARED_FLAG_SPRINTING;
    }
    p.write_u8(DATA_SHARED_FLAGS_ID);
    p.write_varint(ENTITY_DATA_SERIALIZER_BYTE);
    p.write_u8(flags);

    // DATA_POSE (Pose): crouching while sneaking, else standing.
    p.write_u8(DATA_POSE);
    p.write_varint(ENTITY_DATA_SERIALIZER_POSE);
    p.write_varint(if sneaking { POSE_CROUCHING } else { POSE_STANDING });

    p.write_u8(ENTITY_DATA_EOF); // end of metadata list
    frame(CB_PLAY_SET_ENTITY_DATA, &p.buf)
}

/// A generated chunk column: bedrock floor, stone fill, dirt + grass surface
/// following the noise heightmap, air above. The block data and heightmaps come
/// from `crate::world` and vary per chunk `(cx, cz)`. Light is still sent empty
/// — without a real light engine the client falls back to full brightness.
pub fn level_chunk(cx: i32, cz: i32) -> Bytes {
    let columns = crate::world::chunk_columns(cx, cz);

    let mut p = PacketWriter::new();
    p.write_i32(cx);
    p.write_i32(cz);
    // --- ClientboundLevelChunkPacketData ---
    // Heightmaps: a map of type-id -> packed long[] (ByteBufCodecs.map + LONG_ARRAY).
    p.write_varint(columns.heightmaps.len() as i32);
    for (type_id, longs) in &columns.heightmaps {
        p.write_varint(*type_id);
        p.write_varint(longs.len() as i32);
        for &l in longs {
            p.write_i64(l);
        }
    }
    p.write_varint(columns.blob.len() as i32); // section blob length
    p.write_bytes(&columns.blob);
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

/// ClientboundForgetLevelChunkPacket — tell the client to drop a chunk column it
/// previously received (it left the player's view). The body is a single `long`
/// `ChunkPos` (`FriendlyByteBuf.writeChunkPos` → `ChunkPos.pack`): the X is the
/// low 32 bits and the Z the high 32 bits — `x & 0xFFFFFFFF | (z & 0xFFFFFFFF) << 32`.
pub fn forget_chunk(cx: i32, cz: i32) -> Bytes {
    let mut p = PacketWriter::new();
    let packed = (cx as i64 & 0xFFFF_FFFF) | ((cz as i64 & 0xFFFF_FFFF) << 32);
    p.write_i64(packed);
    frame(CB_PLAY_FORGET_LEVEL_CHUNK, &p.buf)
}

/// `ClientboundBlockUpdatePacket` — a single block changed to `state_id`. Layout
/// (`StreamCodec.composite`): the `BlockPos` (packed long) then the block-state
/// id as a VarInt (`ByteBufCodecs.idMapper(Block.BLOCK_STATE_REGISTRY)`).
pub fn block_update(x: i32, y: i32, z: i32, state: BlockState) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_block_pos(x, y, z);
    p.write_varint(state.get() as i32);
    frame(CB_PLAY_BLOCK_UPDATE, &p.buf)
}

/// `ClientboundBlockChangedAckPacket` — acknowledge a serverbound block-change
/// `sequence` so the client retires its predicted change instead of rolling it
/// back. A single VarInt.
pub fn block_changed_ack(sequence: i32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(sequence);
    frame(CB_PLAY_BLOCK_CHANGED_ACK, &p.buf)
}

/// Pack a section coordinate into a `SectionPos` long (`SectionPos.asLong`:
/// 22-bit x at bit 42, 20-bit y at bit 0, 22-bit z at bit 20).
fn section_pos_as_long(sx: i32, sy: i32, sz: i32) -> i64 {
    let x = (sx as i64) & 0x3F_FFFF;
    let y = (sy as i64) & 0xF_FFFF;
    let z = (sz as i64) & 0x3F_FFFF;
    (x << 42) | (z << 20) | y
}

/// `ClientboundSectionBlocksUpdatePacket` — multiple block changes within one
/// 16³ section. Layout: the `SectionPos` (packed long), a VarInt count, then one
/// VarLong per change `(stateId << 12) | (localX << 8 | localZ << 4 | localY)`
/// (`ClientboundSectionBlocksUpdatePacket.write`; the relative packing comes from
/// `SectionPos.sectionRelativePos`). `changes` are `(localX, localY, localZ,
/// state_id)` with each local coordinate in `0..16`.
#[allow(dead_code)] // builder for batched edits; single-block edits use block_update today.
pub fn section_blocks_update(sx: i32, sy: i32, sz: i32, changes: &[(u8, u8, u8, BlockState)]) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_i64(section_pos_as_long(sx, sy, sz));
    p.write_varint(changes.len() as i32);
    for &(lx, ly, lz, state) in changes {
        let local = ((lx as i64) << 8) | ((lz as i64) << 4) | (ly as i64);
        p.write_varlong(((state.get() as i64) << 12) | local);
    }
    frame(CB_PLAY_SECTION_BLOCKS_UPDATE, &p.buf)
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
    fn animate_layout() {
        let (id, mut r) = unframe(animate(7, ANIMATE_SWING_OFF_HAND));
        assert_eq!(id, CB_PLAY_ANIMATE);
        assert_eq!(r.read_varint().unwrap(), 7); // entity id
        assert_eq!(r.read_u8().unwrap(), 3); // action = swing off hand
    }

    #[test]
    fn set_entity_data_standing_layout() {
        // Neither flag set: flags byte 0, pose STANDING.
        let (id, mut r) = unframe(set_entity_data(7, false, false));
        assert_eq!(id, CB_PLAY_SET_ENTITY_DATA);
        assert_eq!(r.read_varint().unwrap(), 7); // entity id
        // Shared flags entry: index 0, serializer BYTE (0), value 0.
        assert_eq!(r.read_u8().unwrap(), 0); // DATA_SHARED_FLAGS_ID
        assert_eq!(r.read_varint().unwrap(), 0); // serializer BYTE
        assert_eq!(r.read_u8().unwrap(), 0); // no bits set
        // Pose entry: index 6, serializer POSE (20), value STANDING (0).
        assert_eq!(r.read_u8().unwrap(), 6); // DATA_POSE
        assert_eq!(r.read_varint().unwrap(), 20); // serializer POSE
        assert_eq!(r.read_varint().unwrap(), 0); // STANDING
        assert_eq!(r.read_u8().unwrap(), 0xFF); // EOF terminator
    }

    #[test]
    fn set_entity_data_sneaking_and_sprinting_layout() {
        // Both flags: byte has 0x02 | 0x08 = 0x0A, pose CROUCHING (5).
        let (id, mut r) = unframe(set_entity_data(42, true, true));
        assert_eq!(id, CB_PLAY_SET_ENTITY_DATA);
        assert_eq!(r.read_varint().unwrap(), 42);
        assert_eq!(r.read_u8().unwrap(), 0); // DATA_SHARED_FLAGS_ID
        assert_eq!(r.read_varint().unwrap(), 0); // serializer BYTE
        assert_eq!(r.read_u8().unwrap(), 0x0A); // crouching | sprinting
        assert_eq!(r.read_u8().unwrap(), 6); // DATA_POSE
        assert_eq!(r.read_varint().unwrap(), 20); // serializer POSE
        assert_eq!(r.read_varint().unwrap(), 5); // CROUCHING
        assert_eq!(r.read_u8().unwrap(), 0xFF); // EOF terminator
    }

    #[test]
    fn block_update_layout() {
        let (id, mut r) = unframe(block_update(1, 64, -3, BlockState(9)));
        assert_eq!(id, CB_PLAY_BLOCK_UPDATE);
        assert_eq!(r.read_block_pos().unwrap(), (1, 64, -3));
        assert_eq!(r.read_varint().unwrap(), 9); // grass_block default state
    }

    #[test]
    fn block_changed_ack_layout() {
        let (id, mut r) = unframe(block_changed_ack(42));
        assert_eq!(id, CB_PLAY_BLOCK_CHANGED_ACK);
        assert_eq!(r.read_varint().unwrap(), 42);
    }

    #[test]
    fn section_blocks_update_layout() {
        // Two changes in section (1, 4, -1).
        let changes = [(2u8, 3u8, 5u8, BlockState(1)), (15u8, 0u8, 0u8, BlockState(9))];
        let (id, mut r) = unframe(section_blocks_update(1, 4, -1, &changes));
        assert_eq!(id, CB_PLAY_SECTION_BLOCKS_UPDATE);
        assert_eq!(r.read_i64().unwrap(), section_pos_as_long(1, 4, -1));
        assert_eq!(r.read_varint().unwrap(), 2); // count
        // entry 0: (1 << 12) | (2<<8 | 5<<4 | 3)
        let e0 = r.read_varlong().unwrap();
        assert_eq!(e0 >> 12, 1); // state id
        assert_eq!(e0 & 0xFFF, (2 << 8) | (5 << 4) | 3); // local x/z/y packing
        // entry 1: (9 << 12) | (15<<8 | 0<<4 | 0)
        let e1 = r.read_varlong().unwrap();
        assert_eq!(e1 >> 12, 9);
        assert_eq!(e1 & 0xFFF, 15 << 8);
    }

    #[test]
    fn forget_chunk_layout() {
        // Body is a single long ChunkPos: x in the low 32 bits, z in the high.
        let (id, mut r) = unframe(forget_chunk(3, -2));
        assert_eq!(id, CB_PLAY_FORGET_LEVEL_CHUNK);
        let packed = r.read_i64().unwrap();
        assert_eq!((packed & 0xFFFF_FFFF) as i32, 3); // x
        assert_eq!((packed >> 32) as i32, -2); // z (sign-extended high half)
    }

    #[test]
    fn set_time_layout_with_clock() {
        // gameTime, then one clock update: overworld id 0, totalTicks (VarLong),
        // partialTick, rate.
        let clock = ClockUpdate {
            clock_id: WORLD_CLOCK_OVERWORLD,
            total_ticks: 13_000,
            partial_tick: 0.25,
            rate: 1.0,
        };
        let (id, mut r) = unframe(set_time(123_456, &[clock]));
        assert_eq!(id, CB_PLAY_SET_TIME);
        assert_eq!(r.read_i64().unwrap(), 123_456); // gameTime (fixed LONG)
        assert_eq!(r.read_varint().unwrap(), 1); // map count
        assert_eq!(r.read_varint().unwrap(), 0); // clock holder id (overworld)
        assert_eq!(r.read_varlong().unwrap(), 13_000); // totalTicks (VarLong)
        assert_eq!(r.read_f32().unwrap(), 0.25); // partialTick
        assert_eq!(r.read_f32().unwrap(), 1.0); // rate
    }

    #[test]
    fn set_time_layout_empty_map_is_gametime_only() {
        // The periodic 1 s resync sends gameTime with a zero-length clock map.
        let (id, mut r) = unframe(set_time(987_654_321, &[]));
        assert_eq!(id, CB_PLAY_SET_TIME);
        assert_eq!(r.read_i64().unwrap(), 987_654_321);
        assert_eq!(r.read_varint().unwrap(), 0); // empty map
    }

    #[test]
    fn set_time_frozen_clock_sends_zero_rate() {
        // Frozen daylight is signalled by rate == 0.0 (not a negative dayTime).
        let clock = ClockUpdate {
            clock_id: WORLD_CLOCK_OVERWORLD,
            total_ticks: 6_000,
            partial_tick: 0.0,
            rate: 0.0,
        };
        let (_id, mut r) = unframe(set_time(0, &[clock]));
        assert_eq!(r.read_i64().unwrap(), 0);
        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_varint().unwrap(), 0);
        assert_eq!(r.read_varlong().unwrap(), 6_000);
        assert_eq!(r.read_f32().unwrap(), 0.0);
        assert_eq!(r.read_f32().unwrap(), 0.0); // rate 0 ⇒ paused
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
