//! Serverbound Play packet decoding: turn a framed packet `(id, body)` into the
//! `Serverbound` message the simulation understands. Pure and synchronous — no
//! I/O — so the layout transcriptions sit apart from the task plumbing in
//! `play_io`.

use crate::protocol::buffer::PacketReader;
use crate::sim::bridge::Serverbound;

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
// PlayerAction sits between PlayerAbilities and PlayerCommand in
// SERVERBOUND_TEMPLATE (line 102 → 41); it carries block-dig actions.
const SB_PLAY_PLAYER_ACTION: i32 = 41;
const SB_PLAY_PLAYER_COMMAND: i32 = 42;
// PlayerInput follows PlayerCommand in SERVERBOUND_TEMPLATE (line 104 → 43); it
// carries the `Input` flags byte that reports crouch in 26.2.
const SB_PLAY_PLAYER_INPUT: i32 = 43;
const SB_PLAY_SWING: i32 = 63;
// UseItemOn (block place) follows TestInstanceBlockAction in SERVERBOUND_TEMPLATE
// (line 127 → 66).
const SB_PLAY_USE_ITEM_ON: i32 = 66;
// Inventory ids: SetCarriedItem (53), SetCreativeModeSlot (56).
const SB_PLAY_SET_CARRIED_ITEM: i32 = 53;
const SB_PLAY_SET_CREATIVE_MODE_SLOT: i32 = 56;

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
