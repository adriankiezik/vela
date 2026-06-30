//! Per-connection pre-Play state machine: handshake -> status | (login ->
//! configuration -> Play handoff).
//!
//! Unencrypted framing (offline mode). A plain frame is:
//!   VarInt(length) | VarInt(packet_id) | body...
//! where `length` covers the id plus the body.
//!
//! Compression *is* negotiated, mid-Login, exactly as vanilla does: just before
//! `ClientboundLoginFinished` the server sends `ClientboundLoginCompressionPacket`
//! (login id 3) carrying the `network-compression-threshold`. That packet itself
//! goes out uncompressed; from the byte after it, both directions switch to the
//! compressed layout. We track this with a local `compression: Option<i32>` that
//! flips from `None` to `Some(threshold)` at that point and is threaded into
//! every `read_frame`/`send_packet`, then handed to `play`.
//!
//! These states are strictly request/response against a single socket, so they
//! run inline here and write straight to the stream. Once the client reaches
//! Play, `play_io::play` takes over and bridges the connection to the simulation.

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
use crate::registry;
use crate::sim::bridge::ToSim;

use super::crypto::{self, AuthError, AuthProfile, ProfileProperty};
use super::frame::{read_frame, send_packet};
use super::play_io::play;
use super::stream::NetStream;

// --- Login --- (clientbound ids in `LoginProtocols` registration order:
// disconnect=0, hello=1, login_finished=2, login_compression=3)
const CB_LOGIN_DISCONNECT: i32 = 0;
const CB_LOGIN_HELLO: i32 = 1;
const CB_LOGIN_FINISHED: i32 = 2;
const CB_LOGIN_COMPRESSION: i32 = 3;
// Serverbound: hello=0, key=1, custom_query_answer=2, login_acknowledged=3.
const SB_LOGIN_HELLO: i32 = 0;
const SB_LOGIN_KEY: i32 = 1;
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
    stream: TcpStream,
    peer: std::net::SocketAddr,
    to_sim: mpsc::Sender<ToSim>,
    config: Arc<ServerConfig>,
) -> io::Result<()> {
    // Wrapped so the framing code is oblivious to whether the AES-CFB8 stream
    // cipher gets installed mid-Login (online mode). Starts plain.
    let mut stream = NetStream::plain(stream);
    let mut state = State::Handshake;
    let mut profile_name = String::new();
    let mut profile_uuid = Uuid::nil();
    // `None` until the compression packet is sent mid-Login, then the threshold.
    let mut compression: Option<i32> = None;
    // Set when an online-mode `ClientboundHello` goes out; the username we are
    // awaiting a `ServerboundKey` for, plus the verify token we challenged with.
    let mut requested_name = String::new();
    let mut verify_token: Option<[u8; 4]> = None;

    loop {
        let (packet_id, mut reader) = match read_frame(&mut stream, compression).await? {
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
                send_packet(&mut stream, 0x00, &w.buf, compression).await?;
            }

            // Status: ping -> echo the same i64 back as pong
            (State::Status, 0x01) => {
                let payload = reader.read_i64()?;
                let mut w = PacketWriter::new();
                w.write_i64(payload);
                send_packet(&mut stream, 0x01, &w.buf, compression).await?;
                return Ok(()); // client closes after pong
            }

            // Login: ServerboundHelloPacket(name, uuid).
            //
            // Online mode (`online-mode=true`): mirror vanilla's `handleHello`
            // and begin encryption — send a `ClientboundHello` carrying our RSA
            // public key and a verify token, then await `ServerboundKey`.
            //
            // Offline mode: there is no key exchange, so go straight to finishing
            // login with an offline profile.
            (State::Login, SB_LOGIN_HELLO) => {
                let name = reader.read_utf(16)?;
                let _client_uuid = reader.read_uuid()?;
                info!(%peer, %name, "login hello");

                if config.properties.online_mode() {
                    let token = crypto::new_verify_token();
                    let keys = crypto::server_keys();
                    let mut w = PacketWriter::new();
                    w.write_utf(""); // serverId — empty on a modern server
                    w.write_byte_array(keys.public_der()); // X.509 SPKI public key
                    w.write_byte_array(&token); // verify token (challenge)
                    w.write_bool(true); // shouldAuthenticate
                    send_packet(&mut stream, CB_LOGIN_HELLO, &w.buf, compression).await?;
                    requested_name = name;
                    verify_token = Some(token);
                    // Stay in Login awaiting the ServerboundKey.
                } else {
                    let uuid = offline_uuid(&name);
                    finish_login(
                        &mut stream,
                        uuid,
                        &name,
                        &[],
                        &config,
                        &mut compression,
                        peer,
                    )
                    .await?;
                    profile_name = name;
                    profile_uuid = uuid;
                    // Stay in Login until the client acknowledges.
                }
            }

            // Login: ServerboundKeyPacket — the client's RSA-wrapped shared
            // secret and verify token. Decrypt, validate the challenge, install
            // the stream cipher, authenticate against Mojang, then finish login.
            // Mirrors vanilla's `handleKey`.
            (State::Login, SB_LOGIN_KEY) => {
                let encrypted_secret = reader.read_byte_array(256)?;
                let encrypted_token = reader.read_byte_array(256)?;
                let token = match verify_token.take() {
                    Some(t) => t,
                    None => return Ok(()), // unexpected Key (no Hello issued)
                };

                // Decrypt + validate the key exchange. A failure here is a
                // protocol error: vanilla throws *before* encryption is set up
                // and drops the connection, so just close — the cipher is not
                // installed on either side, and there is nothing useful to send.
                let secret = match decrypt_key_exchange(&encrypted_secret, &encrypted_token, &token)
                {
                    Ok(secret) => secret,
                    Err(_) => {
                        warn!(%peer, "key exchange failed");
                        return Ok(());
                    }
                };

                // Install the stream cipher NOW, before authenticating. The
                // client enabled its own cipher the instant it sent the Key, so
                // everything we send from here — including an auth-failure
                // disconnect — must be encrypted or the client decrypts plaintext
                // into garbage and shows a blank "Disconnected" screen. Mirrors
                // vanilla `setupEncryption` running ahead of the auth thread.
                stream = stream.enable_encryption(&secret);

                let profile = match resolve_profile(&secret, &requested_name, &config, peer).await {
                    Ok(profile) => {
                        info!(%peer, name = %profile.name, uuid = %profile.uuid, "authenticated");
                        profile
                    }
                    Err(e) => {
                        let reason = match e {
                            AuthError::Unverified => "multiplayer.disconnect.unverified_username",
                            AuthError::Unavailable => "multiplayer.disconnect.authservers_down",
                            // The crypto already succeeded above, so these cannot
                            // surface here; treat any straggler as a drop.
                            AuthError::BadVerifyToken | AuthError::Crypt => {
                                warn!(%peer, "key exchange failed");
                                return Ok(());
                            }
                        };
                        warn!(%peer, name = %requested_name, reason, "login refused");
                        // The cipher is installed, so this goes out encrypted —
                        // which is exactly what the client now expects to read.
                        login_disconnect(&mut stream, reason, compression).await?;
                        return Ok(());
                    }
                };

                finish_login(
                    &mut stream,
                    profile.uuid,
                    &profile.name,
                    &profile.properties,
                    &config,
                    &mut compression,
                    peer,
                )
                .await?;
                profile_uuid = profile.uuid;
                profile_name = profile.name;
                // Stay in Login until the client acknowledges.
            }

            // Login: ServerboundLoginAcknowledged -> enter configuration
            (State::Login, SB_LOGIN_ACK) => {
                info!(%peer, "login acknowledged -> configuration");
                state = State::Configuration;
                begin_configuration(&mut stream, compression).await?;
            }

            // Configuration: client tells us which data packs it has. We claim
            // the vanilla core pack, so now stream the synced registries (entry
            // ids only, data absent) and hand off to play.
            (State::Configuration, SB_CFG_SELECT_KNOWN_PACKS) => {
                debug!(%peer, "client known packs received");
                send_registry_data(&mut stream, compression).await?;
                send_tags(&mut stream, compression).await?;
                send_packet(&mut stream, CB_CFG_FINISH, &[], compression).await?;
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
                return play(rd, wr, peer, profile_uuid, profile_name, to_sim, compression).await;
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

/// Finish login: negotiate compression (vanilla does this in
/// `verifyLoginAndFinishConnectionSetup`, the last UNcompressed frame — both
/// sides switch immediately after) and then send `ClientboundLoginFinished`.
///
/// Shared by the offline path (straight after Hello) and the online path (after
/// the key exchange + Mojang auth), so the ordering — compression, then the
/// finished packet, both encrypted when online — matches vanilla exactly.
async fn finish_login<W: AsyncWrite + Unpin>(
    w: &mut W,
    uuid: Uuid,
    name: &str,
    properties: &[ProfileProperty],
    config: &ServerConfig,
    compression: &mut Option<i32>,
    peer: std::net::SocketAddr,
) -> io::Result<()> {
    let threshold = config.properties.network_compression_threshold();
    if threshold >= 0 && compression.is_none() {
        let mut c = PacketWriter::new();
        c.write_varint(threshold);
        send_packet(w, CB_LOGIN_COMPRESSION, &c.buf, *compression).await?;
        *compression = Some(threshold);
        debug!(%peer, threshold, "compression enabled");
    }
    send_login_finished(w, uuid, name, properties, *compression).await
}

/// ClientboundLoginFinished: the player's GameProfile plus a chat session id.
/// In offline mode the profile carries no signed properties; online it carries
/// the signed skin/cape properties from `hasJoined`.
async fn send_login_finished<W: AsyncWrite + Unpin>(
    w: &mut W,
    uuid: Uuid,
    name: &str,
    properties: &[ProfileProperty],
    compression: Option<i32>,
) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_uuid(uuid); // GameProfile.id
    p.write_utf(name); // GameProfile.name (<=16)
    p.write_varint(properties.len() as i32); // GameProfile.properties
    for prop in properties {
        p.write_utf(&prop.name);
        p.write_utf(&prop.value);
        p.write_optional_utf(prop.signature.as_deref());
    }
    p.write_uuid(uuid); // sessionId
    send_packet(w, CB_LOGIN_FINISHED, &p.buf, compression).await
}

/// A `ClientboundLoginDisconnectPacket` carrying a translatable reason key.
/// In the login state the component is JSON (not network NBT), so we send a
/// minimal `{"translate":"<key>"}` object.
async fn login_disconnect<W: AsyncWrite + Unpin>(
    w: &mut W,
    translate_key: &str,
    compression: Option<i32>,
) -> io::Result<()> {
    let json = format!("{{\"translate\":\"{translate_key}\"}}");
    let mut p = PacketWriter::new();
    p.write_utf(&json);
    send_packet(w, CB_LOGIN_DISCONNECT, &p.buf, compression).await
}

/// Decrypt the client's RSA-wrapped shared secret and verify token and validate
/// the challenge. Returns the 16-byte AES secret. A failure is a pre-encryption
/// protocol error (`BadVerifyToken`/`Crypt`); vanilla `handleKey` throws here
/// before any cipher is installed. Mirrors the cryptographic core of `handleKey`.
fn decrypt_key_exchange(
    encrypted_secret: &[u8],
    encrypted_token: &[u8],
    expected_token: &[u8; 4],
) -> Result<[u8; 16], AuthError> {
    let keys = crypto::server_keys();

    // The verify token must round-trip through our private key unchanged.
    let token = keys.decrypt(encrypted_token)?;
    if token != expected_token {
        return Err(AuthError::BadVerifyToken);
    }

    // The shared secret is a 16-byte AES-128 key.
    let secret_bytes = keys.decrypt(encrypted_secret)?;
    secret_bytes
        .as_slice()
        .try_into()
        .map_err(|_| AuthError::Crypt)
}

/// Compute the server-id hash from the now-known shared secret and resolve the
/// authenticated profile via Mojang's `hasJoined`. Runs *after* the stream
/// cipher is installed, so its `Unverified`/`Unavailable` failures are reported
/// to the client over the encrypted connection — matching vanilla's auth thread.
async fn resolve_profile(
    secret: &[u8; 16],
    name: &str,
    config: &ServerConfig,
    peer: std::net::SocketAddr,
) -> Result<AuthProfile, AuthError> {
    let keys = crypto::server_keys();
    let server_hash = crypto::server_id_hash("", secret, keys.public_der());

    // `prevent-proxy-connections` pins the auth to the client's IP, matching
    // vanilla's optional `&ip=` parameter.
    let ip = if config.properties.prevent_proxy_connections() {
        Some(peer.ip().to_string())
    } else {
        None
    };

    // The HTTP call is blocking; keep it off the async runtime.
    let name = name.to_string();
    tokio::task::spawn_blocking(move || crypto::has_joined(&name, &server_hash, ip.as_deref()))
        .await
        .map_err(|_| AuthError::Unavailable)?
}

/// On entering configuration: announce the server brand (so the client's F3
/// debug screen shows "Vela" instead of "null"), advertise enabled features and
/// the core known pack. Registry data follows once the client echoes the known
/// packs back. Mirrors vanilla's `startConfiguration`, which sends the brand
/// first.
async fn begin_configuration<W: AsyncWrite + Unpin>(
    w: &mut W,
    compression: Option<i32>,
) -> io::Result<()> {
    send_brand(w, compression).await?;

    let mut feats = PacketWriter::new();
    feats.write_varint(registry::ENABLED_FEATURES.len() as i32);
    for f in registry::ENABLED_FEATURES {
        feats.write_identifier(f);
    }
    send_packet(w, CB_CFG_UPDATE_FEATURES, &feats.buf, compression).await?;

    let mut packs = PacketWriter::new();
    packs.write_varint(1);
    packs.write_utf(registry::KNOWN_PACK_NAMESPACE);
    packs.write_utf(registry::KNOWN_PACK_ID);
    packs.write_utf(registry::KNOWN_PACK_VERSION);
    send_packet(w, CB_CFG_SELECT_KNOWN_PACKS, &packs.buf, compression).await
}

/// ClientboundCustomPayloadPacket carrying a `BrandPayload` on the
/// `minecraft:brand` channel. The body is the channel identifier followed by a
/// single UTF string — the brand the client surfaces on its F3 debug screen.
/// Vanilla sends "vanilla" here; we send "Vela".
async fn send_brand<W: AsyncWrite + Unpin>(w: &mut W, compression: Option<i32>) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_identifier("minecraft:brand");
    p.write_utf("Vela");
    send_packet(w, CB_CFG_CUSTOM_PAYLOAD, &p.buf, compression).await
}

/// One ClientboundRegistryData packet per synced registry. Each entry is its
/// id with absent NBT (`Optional` = false), so the client fills the definition
/// from its own copy of the shared core pack.
async fn send_registry_data<W: AsyncWrite + Unpin>(
    w: &mut W,
    compression: Option<i32>,
) -> io::Result<()> {
    for (registry_id, entries) in registry::SYNCED {
        let mut p = PacketWriter::new();
        p.write_identifier(registry_id);
        p.write_varint(entries.len() as i32);
        for entry in *entries {
            p.write_identifier(entry);
            p.write_bool(false); // data absent
        }
        send_packet(w, CB_CFG_REGISTRY_DATA, &p.buf, compression).await?;
    }
    Ok(())
}

/// ClientboundUpdateTags: a map of registry -> (tag name -> entry ids). Every
/// tag the client requires must be present or it aborts configuration; the
/// `minecraft:block` and `minecraft:item` registries carry real member ids
/// (block / item registry indices), the rest stay empty. See `registry::tags`.
async fn send_tags<W: AsyncWrite + Unpin>(w: &mut W, compression: Option<i32>) -> io::Result<()> {
    let mut p = PacketWriter::new();
    p.write_varint(crate::registry::tags::TAGS.len() as i32);
    for registry in crate::registry::tags::TAGS {
        p.write_identifier(registry.registry);
        p.write_varint(registry.tags.len() as i32);
        for tag in registry.tags {
            p.write_identifier(tag.name);
            p.write_varint(tag.ids.len() as i32);
            for id in tag.ids {
                p.write_varint(*id);
            }
        }
    }
    send_packet(w, CB_CFG_UPDATE_TAGS, &p.buf, compression).await
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
