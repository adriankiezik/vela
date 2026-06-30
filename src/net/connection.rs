//! Per-connection pre-Play state machine: handshake -> status | (login ->
//! configuration -> Play handoff).
//!
//! Uncompressed, unencrypted framing (offline mode). A frame is:
//!   VarInt(length) | VarInt(packet_id) | body...
//! where `length` covers the id plus the body. We never negotiate compression
//! (`SetCompression`) or encryption, so every frame stays in this plain form
//! for the whole session.
//!
//! These states are strictly request/response against a single socket, so they
//! run inline here and write straight to the stream. Once the client reaches
//! Play, `play::play` takes over and bridges the connection to the simulation.

use std::sync::Arc;

use serde::Serialize;
use tokio::io::{self, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::ServerConfig;
use crate::protocol::buffer::PacketWriter;
use crate::protocol::uuid::offline_uuid;
use crate::protocol::{Intent, State, PROTOCOL_VERSION, VERSION_NAME};
use crate::registries;
use crate::sim::bridge::ToSim;

use super::frame::{read_frame, send_packet};
use super::play::play;

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

/// Drive a fresh connection through handshake, status/login, and configuration.
/// On reaching Play it hands off to `play`, passing the ingress sender so the
/// connection can register with the simulation.
pub async fn handle(
    mut stream: TcpStream,
    peer: std::net::SocketAddr,
    to_sim: mpsc::Sender<ToSim>,
    config: Arc<ServerConfig>,
) -> io::Result<()> {
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

            // Status: request -> respond with server list JSON. With
            // `enable-status=false` vanilla never answers, so we just close.
            (State::Status, 0x00) => {
                if !config.properties.enable_status() {
                    debug!(%peer, "status disabled; closing");
                    return Ok(());
                }
                let json = status_json(&config);
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
                return play(rd, wr, peer, profile_uuid, profile_name, to_sim).await;
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

#[derive(Serialize)]
struct StatusResponse {
    version: VersionInfo,
    players: Players,
    description: Description,
    /// `data:image/png;base64,…` from `server-icon.png`; omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    favicon: Option<String>,
}

#[derive(Serialize)]
struct VersionInfo {
    name: String,
    protocol: i32,
}

#[derive(Serialize)]
struct Players {
    max: i32,
    online: u32,
}

#[derive(Serialize)]
struct Description {
    text: String,
}

/// The JSON shown in the multiplayer server list. MOTD, max players, and the
/// favicon come from the loaded config (`server.properties` / `server-icon.png`).
fn status_json(config: &ServerConfig) -> String {
    let resp = StatusResponse {
        version: VersionInfo {
            name: format!("Vela {VERSION_NAME}"),
            protocol: PROTOCOL_VERSION,
        },
        players: Players {
            max: config.properties.max_players(),
            online: 0,
        },
        description: Description {
            text: config.properties.motd().to_string(),
        },
        favicon: config.favicon.clone(),
    };
    serde_json::to_string(&resp).expect("status serializes")
}
