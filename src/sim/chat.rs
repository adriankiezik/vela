//! Chat: the three clientbound chat channels (`PlayerChat`, `SystemChat`,
//! `DisguisedChat`), the chat-type registry ids the client decorates against,
//! and the message-signing chain (`ChatSession`).
//!
//! ## Chat types
//!
//! A clientbound chat packet carries the *content* and a bound **chat type**; the
//! client applies the decoration ("<name> msg", "[name] msg", …) from its copy of
//! the `minecraft:chat_type` registry. The registry is synced during
//! configuration (known-packs passthrough), so we only send the numeric holder
//! id. The ids below are the registration order Vela advertises in
//! `registry::mod` — **alphabetical**, which is the order that fixes the wire
//! ids the client assigns.
//!
//! ## Message signing (`ChatSession` / `SignedMessageChain` / `MessageSignature`)
//!
//! In 26.2 a player publishes a [`ChatSession`] (`ServerboundChatSessionUpdate`):
//! a session id plus a `ProfilePublicKey` that signs each message, forming a
//! [`SignedMessageChain`] rooted at `(profileId, sessionId)` whose link index
//! advances per signed message. We decode and store the session, and decode the
//! per-message `MessageSignature`, so the chain *structure* is modelled 1:1.
//!
//! **Stubbed, clearly:** we do not (yet) *verify* signatures (that needs the
//! yggdrasil service key to validate the profile key, and the profile key to
//! validate each message), nor do we *forward* them — a receiving client can
//! only verify a forwarded signature if it also holds the sender's public key,
//! which the server distributes via `PlayerInfoUpdate`'s `INITIALIZE_CHAT`
//! action (not yet implemented). So we broadcast **unsigned** `PlayerChat`
//! (signature absent, link index 0), exactly as a server with secure chat
//! disabled does — Vela advertises `enforces_secure_chat = false` in the join
//! packet, so this is consistent and the client displays every message.

use bevy_ecs::prelude::*;
use bytes::Bytes;
use uuid::Uuid;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;
use crate::protocol::nbt::{write_network, Nbt};

// Clientbound Play ids (registration order, decompiled `GameProtocols`):
// DISGUISED_CHAT (line 166 → 33), PLAYER_CHAT (line 198 → 65).
const CB_PLAY_DISGUISED_CHAT: i32 = 33;
const CB_PLAY_PLAYER_CHAT: i32 = 65;
// COMMAND_SUGGESTIONS is index 15 (bundle delimiter = 0, then alphabetical).
const CB_PLAY_COMMAND_SUGGESTIONS: i32 = 15;

// `minecraft:chat_type` holder ids, in the alphabetical registration order Vela
// syncs (see `registry::mod`). The client decorates the content with these.
/// `minecraft:chat` — plain "<name> message".
pub const CHAT_TYPE_CHAT: i32 = 0;
/// `minecraft:emote_command` — "* name message" (used by `/me`).
pub const CHAT_TYPE_EMOTE_COMMAND: i32 = 1;
/// `minecraft:msg_command_incoming` — "name whispers to you: message".
pub const CHAT_TYPE_MSG_COMMAND_INCOMING: i32 = 2;
/// `minecraft:msg_command_outgoing` — "You whisper to name: message".
pub const CHAT_TYPE_MSG_COMMAND_OUTGOING: i32 = 3;
/// `minecraft:say_command` — "[name] message" (used by `/say`).
pub const CHAT_TYPE_SAY_COMMAND: i32 = 4;

/// The stored profile public key that heads a player's message-signing chain,
/// decoded from `ServerboundChatSessionUpdatePacket` (`RemoteChatSession.Data`).
/// Kept verbatim for fidelity; the fields are held for the deferred verify/
/// forward path (validating the key against the yggdrasil service key, then each
/// message against the key, and propagating it via `PlayerInfoUpdate`
/// INITIALIZE_CHAT) — see the module note — so they read as dead code today.
#[allow(dead_code)]
#[derive(Clone)]
pub struct ChatSession {
    pub session_id: Uuid,
    /// Key expiry, epoch milliseconds.
    pub expires_at: i64,
    /// X.509-encoded RSA public key.
    pub public_key: Vec<u8>,
    /// Mojang's signature over the key (unverified — see module note).
    pub key_signature: Vec<u8>,
}

/// Per-player chat bookkeeping. `session` is the head of the signing chain (set
/// when the client publishes its session); `global_index` is this player's
/// receive-side `ClientboundPlayerChatPacket.globalIndex` counter (vanilla
/// `ServerGamePacketListenerImpl.nextChatIndex`), incremented once per player
/// chat message delivered *to* this player.
#[derive(Component, Default)]
pub struct ChatState {
    pub session: Option<ChatSession>,
    pub global_index: i32,
}

/// `ClientboundPlayerChatPacket` — a player-authored chat message the client
/// decorates via `chat_type`. Layout (`ClientboundPlayerChatPacket.write`):
/// `globalIndex` (VarInt), `sender` (UUID), `index` (VarInt link index),
/// nullable `signature` (256 raw bytes), the `SignedMessageBody.Packed`
/// (`content` string, `timeStamp` long, `salt` long, packed `lastSeen`
/// collection), nullable `unsignedContent` component, the `FilterMask`, then the
/// bound `chat_type` (holder id + name + optional target name).
///
/// We emit the unsigned form (see module note): `signature` absent, `index` 0,
/// empty `lastSeen`, no `unsignedContent`, `PASS_THROUGH` filter.
#[allow(clippy::too_many_arguments)]
pub fn player_chat(
    global_index: i32,
    sender: Uuid,
    content: &str,
    timestamp: i64,
    salt: i64,
    chat_type: i32,
    sender_name: &Nbt,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(global_index);
    p.write_uuid(sender);
    p.write_varint(0); // link index: 0 (unsigned root link)
    p.write_bool(false); // signature: absent (unsigned)
    // SignedMessageBody.Packed:
    p.write_utf(content);
    p.write_i64(timestamp);
    p.write_i64(salt);
    p.write_varint(0); // lastSeen: empty packed collection
    p.write_bool(false); // unsignedContent: absent
    p.write_varint(0); // FilterMask: PASS_THROUGH (enum ordinal 0)
    write_chat_type(&mut p, chat_type, sender_name, None);
    frame(CB_PLAY_PLAYER_CHAT, &p.buf)
}

/// `ClientboundDisguisedChatPacket` — a message not backed by a player signature
/// (server/command chat: `/say`, `/me`, `/tell`). Layout: the `message`
/// component (trusted network NBT) then the bound `chat_type`. The client
/// decorates `message` with the chat type, e.g. `[name] message`.
pub fn disguised_chat(
    message: &Nbt,
    chat_type: i32,
    sender_name: &Nbt,
    target_name: Option<&Nbt>,
) -> Bytes {
    let mut p = PacketWriter::new();
    write_network(&mut p.buf, message);
    write_chat_type(&mut p, chat_type, sender_name, target_name);
    frame(CB_PLAY_DISGUISED_CHAT, &p.buf)
}

/// Write a `ChatType.Bound`: the chat-type `Holder` (VarInt `id + 1`, the
/// registry-reference form — `0` would introduce an inline definition), the
/// sender `name` component, and the optional `targetName` component.
fn write_chat_type(p: &mut PacketWriter, chat_type: i32, name: &Nbt, target_name: Option<&Nbt>) {
    p.write_varint(chat_type + 1);
    write_network(&mut p.buf, name);
    match target_name {
        Some(t) => {
            p.write_bool(true);
            write_network(&mut p.buf, t);
        }
        None => p.write_bool(false),
    }
}

/// `ClientboundCommandSuggestionsPacket` — a reply to a tab-completion request.
/// Layout: `id` (echoed transaction id), `start`/`length` (the `StringRange` the
/// suggestions replace), then a list of entries, each a `text` string and an
/// optional tooltip component (we send no tooltips).
pub fn command_suggestions(id: i32, start: i32, length: i32, suggestions: &[String]) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(id);
    p.write_varint(start);
    p.write_varint(length);
    p.write_varint(suggestions.len() as i32);
    for s in suggestions {
        p.write_utf(s);
        p.write_bool(false); // tooltip: absent
    }
    frame(CB_PLAY_COMMAND_SUGGESTIONS, &p.buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;

    fn unframe(bytes: Bytes) -> (i32, PacketReader) {
        let mut r = PacketReader::new(bytes);
        let _len = r.read_varint().unwrap();
        let id = r.read_varint().unwrap();
        (id, r)
    }

    #[test]
    fn player_chat_unsigned_layout() {
        let uuid = Uuid::from_u128(0x1234);
        let name = super::super::text::text("Steve");
        let (id, mut r) = unframe(player_chat(7, uuid, "hello", 1000, 42, CHAT_TYPE_CHAT, &name));
        assert_eq!(id, CB_PLAY_PLAYER_CHAT);
        assert_eq!(r.read_varint().unwrap(), 7); // globalIndex
        assert_eq!(r.read_uuid().unwrap(), uuid);
        assert_eq!(r.read_varint().unwrap(), 0); // link index
        assert!(!r.read_bool().unwrap()); // signature absent
        assert_eq!(r.read_utf(256).unwrap(), "hello");
        assert_eq!(r.read_i64().unwrap(), 1000); // timestamp
        assert_eq!(r.read_i64().unwrap(), 42); // salt
        assert_eq!(r.read_varint().unwrap(), 0); // lastSeen empty
        assert!(!r.read_bool().unwrap()); // unsignedContent absent
        assert_eq!(r.read_varint().unwrap(), 0); // filter PASS_THROUGH
        assert_eq!(r.read_varint().unwrap(), CHAT_TYPE_CHAT + 1); // holder id+1
    }

    #[test]
    fn disguised_chat_with_target_layout() {
        let msg = super::super::text::text("hi");
        let name = super::super::text::text("Alice");
        let target = super::super::text::text("Bob");
        let (id, mut r) = unframe(disguised_chat(
            &msg,
            CHAT_TYPE_MSG_COMMAND_OUTGOING,
            &name,
            Some(&target),
        ));
        assert_eq!(id, CB_PLAY_DISGUISED_CHAT);
        // message component, then chat type holder id+1, then name, then target.
        assert_eq!(read_component_text(&mut r), "hi");
        assert_eq!(r.read_varint().unwrap(), CHAT_TYPE_MSG_COMMAND_OUTGOING + 1);
        assert_eq!(read_component_text(&mut r), "Alice");
        assert!(r.read_bool().unwrap()); // target present
        assert_eq!(read_component_text(&mut r), "Bob");
    }

    #[test]
    fn command_suggestions_layout() {
        let (id, mut r) = unframe(command_suggestions(3, 6, 2, &["Ada".into(), "Alan".into()]));
        assert_eq!(id, CB_PLAY_COMMAND_SUGGESTIONS);
        assert_eq!(r.read_varint().unwrap(), 3); // transaction id
        assert_eq!(r.read_varint().unwrap(), 6); // start
        assert_eq!(r.read_varint().unwrap(), 2); // length
        assert_eq!(r.read_varint().unwrap(), 2); // entry count
        assert_eq!(r.read_utf(256).unwrap(), "Ada");
        assert!(!r.read_bool().unwrap()); // no tooltip
        assert_eq!(r.read_utf(256).unwrap(), "Alan");
        assert!(!r.read_bool().unwrap());
    }

    /// Read a network-NBT component compound written by `write_network` and
    /// return its `text` field. The nameless-root compound is a single 0x0A tag,
    /// a `String` `text` entry (0x08), then the 0x00 end — enough to assert on.
    fn read_component_text(r: &mut PacketReader) -> String {
        assert_eq!(r.read_u8().unwrap(), 0x0A); // TAG_Compound (nameless root)
        assert_eq!(r.read_u8().unwrap(), 0x08); // TAG_String
        let name_len = r.read_u16().unwrap() as usize;
        let name = String::from_utf8(r.read_bytes(name_len).unwrap()).unwrap();
        assert_eq!(name, "text");
        let val_len = r.read_u16().unwrap() as usize;
        let value = String::from_utf8(r.read_bytes(val_len).unwrap()).unwrap();
        assert_eq!(r.read_u8().unwrap(), 0x00); // TAG_End
        value
    }
}
