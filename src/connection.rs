//! Per-connection state machine: handshake -> status | (login -> configuration -> play).
//!
//! Uncompressed, unencrypted framing (offline mode). A frame is:
//!   VarInt(length) | VarInt(packet_id) | body...
//! where `length` covers the id plus the body. We never negotiate compression
//! (`SetCompression`) or encryption, so every frame stays in this plain form
//! for the whole session.
//!
//! Packet IDs below are the registration-order indices read straight from the
//! decompiled 26.2 protocol builders (`LoginProtocols`, `ConfigurationProtocols`,
//! `GameProtocols`). They are state-specific: the same number means different
//! packets in configuration vs play.

use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use serde::Serialize;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpStream;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::protocol::buffer::{PacketReader, PacketWriter};
use crate::protocol::uuid::offline_uuid;
use crate::protocol::varint::{put_varint, read_varint, varint_len};
use crate::protocol::{Intent, State, PROTOCOL_VERSION, VERSION_NAME};
use crate::registries;

// --- Login ---
const CB_LOGIN_FINISHED: i32 = 2;
const SB_LOGIN_HELLO: i32 = 0;
const SB_LOGIN_ACK: i32 = 3;

// --- Configuration ---
const CB_CFG_CUSTOM_PAYLOAD: i32 = 1;
const CB_CFG_FINISH: i32 = 3;
const CB_CFG_REGISTRY_DATA: i32 = 7;
const CB_CFG_UPDATE_FEATURES: i32 = 12;
const CB_CFG_UPDATE_TAGS: i32 = 13;
const CB_CFG_SELECT_KNOWN_PACKS: i32 = 14;
const SB_CFG_CLIENT_INFORMATION: i32 = 0;
const SB_CFG_FINISH: i32 = 3;
const SB_CFG_KEEP_ALIVE: i32 = 4;
const SB_CFG_SELECT_KNOWN_PACKS: i32 = 7;

// --- Play ---
const CB_PLAY_GAME_EVENT: i32 = 38;
const CB_PLAY_KEEP_ALIVE: i32 = 44;
const CB_PLAY_LEVEL_CHUNK: i32 = 45;
const CB_PLAY_LOGIN: i32 = 49;
const CB_PLAY_PLAYER_POSITION: i32 = 72;
const CB_PLAY_SET_CHUNK_CACHE_CENTER: i32 = 94;
const SB_PLAY_ACCEPT_TELEPORTATION: i32 = 0;
const SB_PLAY_KEEP_ALIVE: i32 = 28;
const SB_PLAY_MOVE_PLAYER_POS: i32 = 30;
const SB_PLAY_MOVE_PLAYER_POS_ROT: i32 = 31;
const SB_PLAY_MOVE_PLAYER_ROT: i32 = 32;
const SB_PLAY_MOVE_PLAYER_STATUS_ONLY: i32 = 33;

/// `ServerboundMovePlayerPacket.FLAG_ON_GROUND` — bit 0 of the trailing flags
/// byte the movement packets carry (bit 1 is horizontal collision, ignored).
const MOVE_FLAG_ON_GROUND: u8 = 1;

/// `GameType.SPECTATOR` — spawns the player floating, so the empty void world
/// is viewable without fall damage or fighting gravity.
const GAME_TYPE_SPECTATOR: u8 = 3;
/// `ClientboundGameEventPacket.LEVEL_CHUNKS_LOAD_START` — tells the client to
/// begin waiting for chunks; the "Loading terrain" screen clears once the
/// chunks around the player arrive.
const GAME_EVENT_LEVEL_CHUNKS_LOAD_START: u8 = 13;

/// Overworld column height: 384 blocks / 16 = 24 sections (min_y -64).
const SECTION_COUNT: usize = 24;
/// Render/simulation radius we advertise. We send exactly this radius of
/// chunks, so the client's terrain wait is fully satisfied.
const VIEW_RADIUS: i32 = 5;

/// Upper bound on a single frame's declared length. Matches vanilla's
/// `MAX_PACKET_SIZE` (2 MiB); anything larger is treated as a protocol error
/// rather than allocated.
const MAX_FRAME_LEN: i32 = 2 * 1024 * 1024;

/// The player's last-known position and orientation in the world. Updated from
/// the serverbound movement packets; the server is otherwise authoritative and
/// does not yet validate or correct these values.
#[derive(Debug, Clone, Copy)]
struct PlayerState {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
}

pub async fn handle(mut stream: TcpStream, peer: std::net::SocketAddr) -> io::Result<()> {
    let mut state = State::Handshake;
    let mut profile_name = String::new();
    let mut profile_uuid = Uuid::nil();

    loop {
        let (packet_id, mut reader) = match read_frame(&mut stream).await? {
            Some(frame) => frame,
            None => return Ok(()), // clean EOF
        };

        match (state, packet_id) {
            // Handshake: ClientIntentionPacket
            (State::Handshake, 0x00) => {
                let protocol = reader.read_varint()?;
                let host = reader.read_utf(255)?;
                let port = reader.read_u16()?;
                let intent = Intent::from_id(reader.read_varint()?);
                info!(%peer, protocol, %host, port, ?intent, "handshake");
                match intent {
                    Some(Intent::Status) => state = State::Status,
                    Some(Intent::Login) | Some(Intent::Transfer) => state = State::Login,
                    None => return Ok(()),
                }
            }

            // Status: request -> respond with server list JSON
            (State::Status, 0x00) => {
                let json = status_json();
                let mut w = PacketWriter::new();
                w.write_utf(&json);
                send_packet(&mut stream, 0x00, &w.buf).await?;
            }

            // Status: ping -> echo the same i64 back as pong
            (State::Status, 0x01) => {
                let payload = reader.read_i64()?;
                let mut w = PacketWriter::new();
                w.write_i64(payload);
                send_packet(&mut stream, 0x01, &w.buf).await?;
                return Ok(()); // client closes after pong
            }

            // Login: ServerboundHelloPacket(name, uuid) -> ClientboundLoginFinished
            (State::Login, SB_LOGIN_HELLO) => {
                let name = reader.read_utf(16)?;
                let _client_uuid = reader.read_uuid()?;
                let uuid = offline_uuid(&name);
                info!(%peer, %name, %uuid, "login hello");

                send_login_finished(&mut stream, uuid, &name).await?;
                profile_name = name;
                profile_uuid = uuid;
                // Stay in Login until the client acknowledges.
            }

            // Login: ServerboundLoginAcknowledged -> enter configuration
            (State::Login, SB_LOGIN_ACK) => {
                info!(%peer, "login acknowledged -> configuration");
                state = State::Configuration;
                begin_configuration(&mut stream).await?;
            }

            // Configuration: client tells us which data packs it has. We claim
            // the vanilla core pack, so now stream the synced registries (entry
            // ids only, data absent) and hand off to play.
            (State::Configuration, SB_CFG_SELECT_KNOWN_PACKS) => {
                debug!(%peer, "client known packs received");
                send_registry_data(&mut stream).await?;
                send_tags(&mut stream).await?;
                send_packet(&mut stream, CB_CFG_FINISH, &[]).await?;
                debug!(%peer, "configuration finished, awaiting ack");
            }

            (State::Configuration, SB_CFG_CLIENT_INFORMATION) => {
                debug!(%peer, "client information received");
            }

            (State::Configuration, SB_CFG_KEEP_ALIVE) => {}

            // Configuration: client acknowledges FinishConfiguration -> play.
            (State::Configuration, SB_CFG_FINISH) => {
                info!(%peer, name = %profile_name, "entering play");
                let (rd, wr) = stream.into_split();
                return play(rd, wr, peer, profile_uuid, profile_name).await;
            }

            // Other configuration packets (custom-payload "brand", cookies,
            // pong, …) are accepted but not acted upon. Ignoring rather than
            // disconnecting keeps the handshake progressing toward play.
            (State::Configuration, id) => {
                debug!(%peer, id = format!("{id:#04x}"), "ignored configuration packet");
            }

            (s, id) => {
                warn!(%peer, ?s, id = format!("{id:#04x}"), "unhandled packet");
                return Ok(());
            }
        }
    }
}

/// ClientboundLoginFinished: the player's GameProfile plus a chat session id.
/// In offline mode the profile carries no signed properties.
async fn send_login_finished<W: AsyncWrite + Unpin>(
    w: &mut W,
    uuid: Uuid,
    name: &str,
) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_uuid(uuid); // GameProfile.id
    p.write_utf(name); // GameProfile.name (<=16)
    p.write_varint(0); // GameProfile.properties: none
    p.write_uuid(uuid); // sessionId
    send_packet(w, CB_LOGIN_FINISHED, &p.buf).await
}

/// On entering configuration: announce the server brand (so the client's F3
/// debug screen shows "Vela" instead of "null"), advertise enabled features and
/// the core known pack. Registry data follows once the client echoes the known
/// packs back. Mirrors vanilla's `startConfiguration`, which sends the brand
/// first.
async fn begin_configuration<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    send_brand(w).await?;

    let mut feats = PacketWriter::new();
    feats.write_varint(registries::ENABLED_FEATURES.len() as i32);
    for f in registries::ENABLED_FEATURES {
        feats.write_identifier(f);
    }
    send_packet(w, CB_CFG_UPDATE_FEATURES, &feats.buf).await?;

    let mut packs = PacketWriter::new();
    packs.write_varint(1);
    packs.write_utf(registries::KNOWN_PACK_NAMESPACE);
    packs.write_utf(registries::KNOWN_PACK_ID);
    packs.write_utf(registries::KNOWN_PACK_VERSION);
    send_packet(w, CB_CFG_SELECT_KNOWN_PACKS, &packs.buf).await
}

/// ClientboundCustomPayloadPacket carrying a `BrandPayload` on the
/// `minecraft:brand` channel. The body is the channel identifier followed by a
/// single UTF string — the brand the client surfaces on its F3 debug screen.
/// Vanilla sends "vanilla" here; we send "Vela".
async fn send_brand<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_identifier("minecraft:brand");
    p.write_utf("Vela");
    send_packet(w, CB_CFG_CUSTOM_PAYLOAD, &p.buf).await
}

/// One ClientboundRegistryData packet per synced registry. Each entry is its
/// id with absent NBT (`Optional` = false), so the client fills the definition
/// from its own copy of the shared core pack.
async fn send_registry_data<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    for (registry_id, entries) in registries::SYNCED {
        let mut p = PacketWriter::new();
        p.write_identifier(registry_id);
        p.write_varint(entries.len() as i32);
        for entry in *entries {
            p.write_identifier(entry);
            p.write_bool(false); // data absent
        }
        send_packet(w, CB_CFG_REGISTRY_DATA, &p.buf).await?;
    }
    Ok(())
}

/// ClientboundUpdateTags: a map of registry -> (tag name -> entry ids). We bind
/// only the tags the client requires, each with an empty id list. Without this
/// the client aborts configuration with "Unbound tags".
async fn send_tags<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_varint(crate::registry_tags::EMPTY_TAGS.len() as i32);
    for (registry_id, tags) in crate::registry_tags::EMPTY_TAGS {
        p.write_identifier(registry_id);
        p.write_varint(tags.len() as i32);
        for tag in *tags {
            p.write_identifier(tag);
            p.write_varint(0); // empty id list
        }
    }
    send_packet(w, CB_CFG_UPDATE_TAGS, &p.buf).await
}

/// The play phase: send the join sequence, then service keep-alives (so the
/// client doesn't time out) while draining inbound packets. Reading and the
/// keep-alive timer run concurrently over the split stream halves.
async fn play(
    rd: OwnedReadHalf,
    mut wr: tokio::net::tcp::OwnedWriteHalf,
    peer: std::net::SocketAddr,
    _uuid: Uuid,
    name: String,
) -> io::Result<()> {
    send_play_login(&mut wr).await?;
    // GameEvent first so the client enters its "waiting for chunks" state.
    send_game_event(&mut wr, GAME_EVENT_LEVEL_CHUNKS_LOAD_START, 0.0).await?;
    send_set_chunk_center(&mut wr, 0, 0).await?;
    for cx in -VIEW_RADIUS..=VIEW_RADIUS {
        for cz in -VIEW_RADIUS..=VIEW_RADIUS {
            send_empty_chunk(&mut wr, cx, cz).await?;
        }
    }
    // Synchronize player position (teleport id 1); the client confirms it.
    send_player_position(&mut wr, 1, 0.0, 64.0, 0.0).await?;
    info!(%peer, %name, "play join sequence sent");

    // Track where the client says it is, seeded with the spawn point we just
    // teleported it to. Movement packets below keep this in sync.
    let mut player = PlayerState {
        x: 0.0,
        y: 64.0,
        z: 0.0,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
    };

    // Decode frames in a dedicated task over a buffered reader, handing each one
    // to the select loop through a channel. This keeps `read_frame` — which is
    // *not* cancellation-safe (it consumes bytes incrementally) — from ever
    // being dropped mid-frame by the keep-alive timer, which would desync the
    // stream. The buffering also collapses the per-byte VarInt reads into far
    // fewer syscalls.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(i32, PacketReader)>(32);
    let reader = tokio::spawn(async move {
        let mut rd = tokio::io::BufReader::new(rd);
        // Stops on clean EOF or a decode error (`read_frame` yields `Ok(None)` /
        // `Err`); the closed channel then tells the loop the client is gone.
        while let Ok(Some(frame)) = read_frame(&mut rd).await {
            if tx.send(frame).await.is_err() {
                break; // select loop is gone
            }
        }
    });

    let mut keepalive = tokio::time::interval(Duration::from_secs(10));
    keepalive.tick().await; // first tick is immediate; skip it
    let mut keepalive_id: i64 = 0;
    // Whether a keep-alive is outstanding. If the next tick arrives before the
    // client has echoed the last one, it has gone unresponsive — disconnect
    // rather than keep a half-open connection alive indefinitely.
    let mut awaiting_keepalive = false;

    let result = loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if awaiting_keepalive {
                    warn!(%peer, %name, "keep-alive timeout");
                    break Ok(());
                }
                keepalive_id = keepalive_id.wrapping_add(1);
                let mut p = PacketWriter::new();
                p.write_i64(keepalive_id);
                if let Err(e) = send_packet(&mut wr, CB_PLAY_KEEP_ALIVE, &p.buf).await {
                    break Err(e);
                }
                awaiting_keepalive = true;
            }
            frame = rx.recv() => {
                match frame {
                    None => {
                        info!(%peer, %name, "client disconnected");
                        break Ok(());
                    }
                    Some((id, mut reader)) => match id {
                        SB_PLAY_KEEP_ALIVE => {
                            if reader.read_i64().unwrap_or(-1) == keepalive_id {
                                awaiting_keepalive = false;
                            }
                        }
                        SB_PLAY_ACCEPT_TELEPORTATION => {
                            let tp = reader.read_varint().unwrap_or(-1);
                            debug!(%peer, teleport_id = tp, "teleport confirmed");
                        }
                        // Movement packets. Each is a subset of position + rotation
                        // + a trailing flags byte (on-ground in bit 0). We update
                        // only the fields the variant actually carries, leaving the
                        // rest at their previous values, mirroring vanilla's
                        // hasPos/hasRot fallbacks.
                        SB_PLAY_MOVE_PLAYER_POS => match read_move(&mut reader, true, false, &mut player) {
                            Ok(()) => debug!(%peer, %name, x = player.x, y = player.y, z = player.z, on_ground = player.on_ground, "move pos"),
                            Err(e) => debug!(%peer, %name, error = %e, "malformed move pos"),
                        },
                        SB_PLAY_MOVE_PLAYER_POS_ROT => match read_move(&mut reader, true, true, &mut player) {
                            Ok(()) => debug!(%peer, %name, x = player.x, y = player.y, z = player.z, yaw = player.yaw, pitch = player.pitch, on_ground = player.on_ground, "move pos+rot"),
                            Err(e) => debug!(%peer, %name, error = %e, "malformed move pos+rot"),
                        },
                        SB_PLAY_MOVE_PLAYER_ROT => match read_move(&mut reader, false, true, &mut player) {
                            Ok(()) => debug!(%peer, %name, yaw = player.yaw, pitch = player.pitch, on_ground = player.on_ground, "move rot"),
                            Err(e) => debug!(%peer, %name, error = %e, "malformed move rot"),
                        },
                        SB_PLAY_MOVE_PLAYER_STATUS_ONLY => match read_move(&mut reader, false, false, &mut player) {
                            Ok(()) => debug!(%peer, %name, on_ground = player.on_ground, "move status"),
                            Err(e) => debug!(%peer, %name, error = %e, "malformed move status"),
                        },
                        // Abilities, chat, etc. — accepted but not yet acted upon.
                        // Ignoring keeps the connection alive.
                        _ => {}
                    },
                }
            }
        }
    };

    reader.abort();
    result
}

/// Decode a `ServerboundMovePlayerPacket` variant into `player`. The wire layout
/// is the present fields in order — position (3 doubles) when `has_pos`, rotation
/// (yaw then pitch, 2 floats) when `has_rot` — followed by a single flags byte
/// whose bit 0 is the on-ground state. Absent fields keep their prior value.
fn read_move(
    reader: &mut PacketReader,
    has_pos: bool,
    has_rot: bool,
    player: &mut PlayerState,
) -> io::Result<()> {
    let (mut x, mut y, mut z) = (player.x, player.y, player.z);
    if has_pos {
        x = reader.read_f64()?;
        y = reader.read_f64()?;
        z = reader.read_f64()?;
    }
    let (mut yaw, mut pitch) = (player.yaw, player.pitch);
    if has_rot {
        yaw = reader.read_f32()?;
        pitch = reader.read_f32()?;
    }
    let flags = reader.read_u8()?;

    player.x = x;
    player.y = y;
    player.z = z;
    player.yaw = yaw;
    player.pitch = pitch;
    player.on_ground = flags & MOVE_FLAG_ON_GROUND != 0;
    Ok(())
}

/// ClientboundLogin — the play "join game" packet. Spawns into a single
/// overworld dimension in spectator mode.
async fn send_play_login<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_i32(1); // entity id
    p.write_bool(false); // hardcore
    p.write_varint(1); // levels: 1 dimension
    p.write_identifier("minecraft:overworld");
    p.write_varint(42); // max players
    p.write_varint(VIEW_RADIUS); // view distance
    p.write_varint(VIEW_RADIUS); // simulation distance
    p.write_bool(false); // reduced debug info
    p.write_bool(true); // show death screen
    p.write_bool(false); // limited crafting
    // CommonPlayerSpawnInfo:
    p.write_varint(1); // dimensionType holder: overworld is registry index 0 -> id+1
    p.write_identifier("minecraft:overworld"); // dimension
    p.write_i64(0); // seed (hashed, cosmetic)
    p.write_u8(GAME_TYPE_SPECTATOR); // game type
    p.write_u8(0xFF); // previous game type = -1 (none)
    p.write_bool(false); // is debug
    p.write_bool(false); // is flat
    p.write_bool(false); // last death location: absent
    p.write_varint(0); // portal cooldown
    p.write_varint(63); // sea level
    p.write_bool(false); // online mode (offline)
    p.write_bool(false); // enforces secure chat
    send_packet(w, CB_PLAY_LOGIN, &p.buf).await
}

async fn send_set_chunk_center<W: AsyncWrite + Unpin>(w: &mut W, x: i32, z: i32) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_varint(x);
    p.write_varint(z);
    send_packet(w, CB_PLAY_SET_CHUNK_CACHE_CENTER, &p.buf).await
}

async fn send_game_event<W: AsyncWrite + Unpin>(w: &mut W, event: u8, param: f32) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_u8(event);
    p.write_f32(param);
    send_packet(w, CB_PLAY_GAME_EVENT, &p.buf).await
}

async fn send_player_position<W: AsyncWrite + Unpin>(
    w: &mut W,
    teleport_id: i32,
    x: f64,
    y: f64,
    z: f64,
) -> io::Result<()> {
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
    send_packet(w, CB_PLAY_PLAYER_POSITION, &p.buf).await
}

/// An all-air chunk column with empty light. Each of the 24 sections is 8 zero
/// bytes: non-empty block count (short 0), fluid count (short 0), a single-value
/// air block-state palette (bits=0, VarInt 0, no data array), and a single-value
/// biome palette (bits=0, VarInt 0).
async fn send_empty_chunk<W: AsyncWrite + Unpin>(w: &mut W, cx: i32, cz: i32) -> io::Result<()> {
    let section_data = [0u8; SECTION_COUNT * 8];

    let mut p = PacketWriter::new();
    p.write_i32(cx);
    p.write_i32(cz);
    // --- ClientboundLevelChunkPacketData ---
    p.write_varint(0); // heightmaps: empty map
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
    send_packet(w, CB_PLAY_LEVEL_CHUNK, &p.buf).await
}

/// Read one frame: VarInt(length) | VarInt(id) | body. Returns `None` on a
/// clean EOF (length VarInt could not start), the packet id and a reader over
/// the body otherwise.
async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<(i32, PacketReader)>> {
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
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame).await?;
    let mut reader = PacketReader::new(Bytes::from(frame));
    let id = reader.read_varint()?;
    Ok(Some((id, reader)))
}

/// Frame and send a packet: VarInt(len) | VarInt(id) | body.
async fn send_packet<W: AsyncWrite + Unpin>(w: &mut W, id: i32, body: &[u8]) -> io::Result<()> {
    let payload_len = varint_len(id) + body.len();
    let mut out = BytesMut::with_capacity(varint_len(payload_len as i32) + payload_len);
    put_varint(&mut out, payload_len as i32);
    put_varint(&mut out, id);
    out.extend_from_slice(body);
    debug!(id = format!("{id:#04x}"), bytes = out.remaining(), "send");
    w.write_all(&out).await?;
    w.flush().await
}

#[derive(Serialize)]
struct StatusResponse {
    version: VersionInfo,
    players: Players,
    description: Description,
}

#[derive(Serialize)]
struct VersionInfo {
    name: String,
    protocol: i32,
}

#[derive(Serialize)]
struct Players {
    max: u32,
    online: u32,
}

#[derive(Serialize)]
struct Description {
    text: String,
}

/// The JSON shown in the multiplayer server list, built from typed structs.
fn status_json() -> String {
    let resp = StatusResponse {
        version: VersionInfo {
            name: format!("Vela {VERSION_NAME}"),
            protocol: PROTOCOL_VERSION,
        },
        players: Players { max: 42, online: 0 },
        description: Description {
            text: "\u{00a7}bVela\u{00a7}r \u{00a7}7- a Rust Minecraft server".to_string(),
        },
    };
    serde_json::to_string(&resp).expect("status serializes")
}
