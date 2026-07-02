//! Serverbound Play packet decoding: turn a framed packet `(id, body)` into the
//! `Serverbound` message the simulation understands. Pure and synchronous — no
//! I/O — so the layout transcriptions sit apart from the task plumbing in
//! `play_io`.

use crate::protocol::buffer::PacketReader;
use crate::sim::bridge::Serverbound;

// Serverbound Play packet ids (registration order, decompiled `GameProtocols`).
const SB_PLAY_ACCEPT_TELEPORTATION: i32 = 0;
// Attack is index 1 in SERVERBOUND_TEMPLATE (right after ACCEPT_TELEPORTATION).
// 26.2 split left-click attacks into their own packet, distinct from Interact.
const SB_PLAY_ATTACK: i32 = 1;
const SB_PLAY_CHAT_COMMAND: i32 = 7;
const SB_PLAY_CHAT_COMMAND_SIGNED: i32 = 8;
const SB_PLAY_CHAT: i32 = 9;
// ChatSessionUpdate follows Chat in SERVERBOUND_TEMPLATE (line 71 → 10); it
// publishes the client's chat session (session id + profile public key).
const SB_PLAY_CHAT_SESSION_UPDATE: i32 = 10;
// ChunkBatchReceived (client's batch ack + sustainable rate) is index 11 in
// SERVERBOUND_TEMPLATE (right after CHAT_SESSION_UPDATE at 10).
const SB_PLAY_CHUNK_BATCH_RECEIVED: i32 = 11;
// ClientCommand (respawn request / stats) is index 12 in SERVERBOUND_TEMPLATE
// (CHUNK_BATCH_RECEIVED at 11, CLIENT_COMMAND at 12).
const SB_PLAY_CLIENT_COMMAND: i32 = 12;
// ClientInformation (client settings resend) is index 14 in SERVERBOUND_TEMPLATE,
// right before CommandSuggestion. Same layout as the Configuration-phase packet
// (`ClientInformation`): readUtf(16) language, `readByte` viewDistance, chatVisibility
// enum, chatColors bool, unsignedByte modelCustomisation, mainHand enum,
// textFilteringEnabled bool, allowsListing bool, particleStatus enum.
const SB_PLAY_CLIENT_INFORMATION: i32 = 14;
// CommandSuggestion (tab-completion request) is index 15 in SERVERBOUND_TEMPLATE
// (ACCEPT_TELEPORTATION..COMMAND_SUGGESTION, with CLIENT_INFORMATION at 14).
const SB_PLAY_COMMAND_SUGGESTION: i32 = 15;
const SB_PLAY_KEEP_ALIVE: i32 = 28;
const SB_PLAY_MOVE_PLAYER_POS: i32 = 30;
const SB_PLAY_MOVE_PLAYER_POS_ROT: i32 = 31;
const SB_PLAY_MOVE_PLAYER_ROT: i32 = 32;
const SB_PLAY_MOVE_PLAYER_STATUS_ONLY: i32 = 33;
// Player-action ids, same registration-order source. `GameProtocols`
// SERVERBOUND_TEMPLATE: PlayerAbilities (line 101 → 40), PlayerCommand
// (line 103 → 42), Swing (line 124 → 63).
const SB_PLAY_PLAYER_ABILITIES: i32 = 40;
// PlayerAction sits between PlayerAbilities and PlayerCommand in
// SERVERBOUND_TEMPLATE (line 102 → 41); it carries block-dig actions.
const SB_PLAY_PLAYER_ACTION: i32 = 41;
const SB_PLAY_PLAYER_COMMAND: i32 = 42;
// PlayerInput follows PlayerCommand in SERVERBOUND_TEMPLATE (line 104 → 43); it
// carries the `Input` flags byte that reports crouch in 26.2.
const SB_PLAY_PLAYER_INPUT: i32 = 43;
// PlayerLoaded follows PlayerInput in SERVERBOUND_TEMPLATE (line 105 → 44); an
// empty packet the client sends once it has loaded the level around the player.
const SB_PLAY_PLAYER_LOADED: i32 = 44;
const SB_PLAY_SWING: i32 = 63;
// UseItemOn (block place) follows TestInstanceBlockAction in SERVERBOUND_TEMPLATE
// (line 127 → 66).
const SB_PLAY_USE_ITEM_ON: i32 = 66;
// Inventory ids: SetCarriedItem (53), SetCreativeModeSlot (56).
const SB_PLAY_SET_CARRIED_ITEM: i32 = 53;
const SB_PLAY_SET_CREATIVE_MODE_SLOT: i32 = 56;
// Menu ids: ContainerClick (18), ContainerClose (19) in SERVERBOUND_TEMPLATE.
const SB_PLAY_CONTAINER_CLICK: i32 = 18;
const SB_PLAY_CONTAINER_CLOSE: i32 = 19;

/// `ServerboundMovePlayerPacket.FLAG_ON_GROUND` — bit 0 of the trailing flags
/// byte the movement packets carry (bit 1 is horizontal collision, ignored).
const MOVE_FLAG_ON_GROUND: u8 = 1;

/// `Input.FLAG_SHIFT` — the crouch bit of the `ServerboundPlayerInputPacket`
/// flags byte (forward 1, backward 2, left 4, right 8, jump 16, shift 32,
/// sprint 64).
const INPUT_FLAG_SHIFT: u8 = 32;

/// Decode a serverbound Play packet into a `Serverbound` the sim understands.
/// Unknown or malformed packets yield `None` and are simply dropped — each
/// frame is its own buffer, so unread trailing fields can't desync the stream.
pub(super) fn decode_play(id: i32, reader: &mut PacketReader) -> Option<Serverbound> {
    match id {
        SB_PLAY_KEEP_ALIVE => Some(Serverbound::KeepAlive(reader.read_i64().ok()?)),
        SB_PLAY_ACCEPT_TELEPORTATION => {
            Some(Serverbound::AcceptTeleport(reader.read_varint().ok()?))
        }
        // ServerboundPlayerLoadedPacket: empty body — the client has loaded the
        // level around the player, ending the post-join/respawn load gate.
        SB_PLAY_PLAYER_LOADED => Some(Serverbound::PlayerLoaded),
        // ServerboundChunkBatchReceivedPacket: a single float, the rate the client
        // can sustain (`desiredChunksPerTick`), acknowledging one chunk batch.
        SB_PLAY_CHUNK_BATCH_RECEIVED => Some(Serverbound::ChunkBatchReceived {
            desired_chunks_per_tick: reader.read_f32().ok()?,
        }),
        SB_PLAY_MOVE_PLAYER_POS => decode_move(reader, true, false),
        SB_PLAY_MOVE_PLAYER_POS_ROT => decode_move(reader, true, true),
        SB_PLAY_MOVE_PLAYER_ROT => decode_move(reader, false, true),
        SB_PLAY_MOVE_PLAYER_STATUS_ONLY => decode_move(reader, false, false),
        // ServerboundChatPacket: message, timestamp (Instant → epoch-millis
        // long), salt, nullable 256-byte signature, then the `LastSeenMessages`
        // acknowledgement window (offset VarInt, fixed 20-bit BitSet = 3 bytes,
        // checksum byte) which we decode and drop. The signing fields feed the
        // message-signing chain in `sim::chat`.
        SB_PLAY_CHAT => {
            let message = reader.read_utf(256).ok()?;
            let timestamp = reader.read_i64().ok()?;
            let salt = reader.read_i64().ok()?;
            let signature = if reader.read_bool().ok()? {
                Some(reader.read_bytes(256).ok()?)
            } else {
                None
            };
            // LastSeenMessages.Update: offset, fixed BitSet(20) = 3 bytes, checksum.
            let _offset = reader.read_varint().ok()?;
            let _acknowledged = reader.read_bytes(3).ok()?;
            let _checksum = reader.read_u8().ok()?;
            Some(Serverbound::Chat {
                message,
                timestamp,
                salt,
                signature,
            })
        }
        // ServerboundChatSessionUpdatePacket → RemoteChatSession.Data: a session
        // UUID then ProfilePublicKey.Data (expiry epoch-millis long, X.509 public
        // key byte-array, Mojang key-signature byte-array).
        SB_PLAY_CHAT_SESSION_UPDATE => {
            let session_id = reader.read_uuid().ok()?;
            let expires_at = reader.read_i64().ok()?;
            let public_key = reader.read_byte_array(512).ok()?;
            let key_signature = reader.read_byte_array(4096).ok()?;
            Some(Serverbound::ChatSessionUpdate {
                session_id,
                expires_at,
                public_key,
                key_signature,
            })
        }
        // ServerboundCommandSuggestionPacket: transaction id (VarInt) + the full
        // partial command line being completed (String, incl. the leading `/`).
        SB_PLAY_COMMAND_SUGGESTION => Some(Serverbound::CommandSuggestion {
            id: reader.read_varint().ok()?,
            command: reader.read_utf(32500).ok()?,
        }),
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
        // ServerboundAttackPacket: a single VarInt — the attacked entity's id.
        SB_PLAY_ATTACK => Some(Serverbound::Attack {
            entity_id: reader.read_varint().ok()?,
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
        // ServerboundPlayerInputPacket: an `Input` flags byte; bit 0x20 = SHIFT
        // (crouch). We only surface the sneak state.
        SB_PLAY_PLAYER_INPUT => Some(Serverbound::PlayerInput {
            sneaking: reader.read_u8().ok()? & INPUT_FLAG_SHIFT != 0,
        }),
        // ServerboundPlayerActionPacket: action (VarInt enum ordinal),
        // blockPos (packed long), direction (unsigned byte 3D-data value),
        // sequence (VarInt). Reference: `ServerboundPlayerActionPacket.<init>`.
        SB_PLAY_PLAYER_ACTION => {
            let action = reader.read_varint().ok()?;
            let (x, y, z) = reader.read_block_pos().ok()?;
            let face = reader.read_u8().ok()? as i32;
            let sequence = reader.read_varint().ok()?;
            Some(Serverbound::PlayerAction {
                action,
                x,
                y,
                z,
                face,
                sequence,
            })
        }
        // ServerboundUseItemOnPacket: hand (VarInt enum ordinal), then the
        // BlockHitResult — blockPos (packed long), direction (VarInt enum
        // ordinal, *not* a byte here, per `FriendlyByteBuf.readBlockHitResult`),
        // cursor x/y/z (floats), inside (bool), worldBorder (bool) — then
        // sequence (VarInt). The world-border flag is read and dropped.
        SB_PLAY_USE_ITEM_ON => {
            let hand = reader.read_varint().ok()?;
            let (x, y, z) = reader.read_block_pos().ok()?;
            let face = reader.read_varint().ok()?;
            let cursor_x = reader.read_f32().ok()?;
            let cursor_y = reader.read_f32().ok()?;
            let cursor_z = reader.read_f32().ok()?;
            let inside = reader.read_bool().ok()?;
            let _world_border = reader.read_bool().ok()?;
            let sequence = reader.read_varint().ok()?;
            Some(Serverbound::UseItemOn {
                hand,
                x,
                y,
                z,
                face,
                cursor_x,
                cursor_y,
                cursor_z,
                inside,
                sequence,
            })
        }
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
        // ServerboundContainerClickPacket: containerId (VarInt), stateId (VarInt),
        // slotNum (short), buttonNum (byte), containerInput (VarInt enum id). The
        // trailing predicted `changedSlots` map and `carriedItem` HashedStack are
        // left unread — the server re-syncs authoritative state after the click,
        // and each frame is its own buffer so the unread tail can't desync.
        SB_PLAY_CONTAINER_CLICK => {
            let container_id = reader.read_varint().ok()?;
            let state_id = reader.read_varint().ok()?;
            let slot = reader.read_u16().ok()? as i16;
            let button = reader.read_u8().ok()? as i8;
            let mode = reader.read_varint().ok()?;
            Some(Serverbound::ContainerClick {
                container_id,
                state_id,
                slot,
                button,
                mode,
            })
        }
        // ServerboundContainerClosePacket: a single VarInt container id.
        SB_PLAY_CONTAINER_CLOSE => Some(Serverbound::ContainerClose {
            container_id: reader.read_varint().ok()?,
        }),
        // ServerboundClientCommandPacket: a single `Action` enum ordinal (VarInt).
        SB_PLAY_CLIENT_COMMAND => Some(Serverbound::ClientCommand {
            action: reader.read_varint().ok()?,
        }),
        // ServerboundClientInformationPacket (`SB_PLAY_CLIENT_INFORMATION`): a client
        // resending its settings mid-session. Vanilla `ServerPlayer.updateOptions`
        // copies `viewDistance` into `requestedViewDistance`; because the effective
        // radius is `getPlayerViewDistance = clamp(requestedViewDistance, 2,
        // serverViewDistance)`, a changed view distance must then re-diff the player's
        // tracked chunks. The body is `language` (String<=16) then the `viewDistance`
        // byte; the remaining options (chat visibility, skin customisation, main hand,
        // …) are decoded past but dropped — only the view distance drives server state.
        SB_PLAY_CLIENT_INFORMATION => {
            let _language = reader.read_utf(16).ok()?;
            let view_distance = reader.read_u8().ok()? as i32;
            Some(Serverbound::ClientInformation { view_distance })
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
