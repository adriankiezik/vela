//! Per-command handlers and their shared helpers. Each `cmd_*` matches the
//! [`Handler`](super::Handler) signature and is wired into [`COMMANDS`](super::COMMANDS).

use bytes::Bytes;
use bevy_ecs::prelude::*;

use crate::protocol::nbt::Nbt;

use crate::sim::bridge::Outbound;
use crate::sim::chat;
use crate::sim::components::{Config, Conn, Control, PlayerId, Profile};
use crate::sim::packets;
use crate::sim::text;

/// `/seed` — `commands.seed.success` with the click-to-copy seed value. The flat
/// world's seed is 0, matching the value sent in the play-login spawn info.
pub(super) fn cmd_seed(_world: &mut World, _sender: Entity, _args: &str) -> Option<Nbt> {
    Some(text::translatable("commands.seed.success", vec![text::copy_on_click("0")]))
}

/// `/list` (and `/list uuids`) — `commands.list.players` with the online count,
/// advertised max, and the comma-joined player list. The `uuids` form appends
/// each player's UUID as `name (uuid)`, mirroring `commands.list.nameAndId`.
pub(super) fn cmd_list(world: &mut World, _sender: Entity, args: &str) -> Option<Nbt> {
    let with_uuids = args.split_whitespace().next() == Some("uuids");
    // The advertised max comes from `server.properties` (same source the join
    // packet uses), read before the player query takes its mutable borrow.
    let max = world.resource::<Config>().join_params().max_players;
    let mut q = world.query::<(&Profile, &PlayerId)>();
    let names: Vec<String> = q
        .iter(world)
        .map(|(profile, pid)| {
            if with_uuids {
                format!("{} ({})", profile.name, pid.0)
            } else {
                profile.name.clone()
            }
        })
        .collect();
    Some(text::translatable(
        "commands.list.players",
        vec![
            text::text(names.len().to_string()),
            text::text(max.to_string()),
            text::text(names.join(", ")),
        ],
    ))
}

/// `/stop` — request a graceful server shutdown. Vanilla replies
/// `commands.stop.stopping` ("Stopping the server") as a broadcast success, then
/// halts; here we raise the shared shutdown flag the run loop watches, so the
/// next tick saves the world (`persistence::shutdown`) and the process exits.
pub(super) fn cmd_stop(world: &mut World, _sender: Entity, _args: &str) -> Option<Nbt> {
    world.resource::<Control>().request_shutdown();
    Some(text::translatable("commands.stop.stopping", vec![]))
}

// --- Chat / messaging commands ----------------------------------------------

/// The sender's display-name component (`{text: name}`) and UUID, or `None` if
/// the entity has no `Profile` (already dropped). Offline profiles carry no
/// styled display name, so a plain literal matches what the tab list shows.
fn sender_name(world: &World, entity: Entity) -> Option<String> {
    world.get::<Profile>(entity).map(|p| p.name.clone())
}

/// Fan a framed packet out to every connected player (including `sender`).
fn broadcast_all(world: &mut World, bytes: Bytes) {
    let mut q = world.query::<&Conn>();
    for conn in q.iter(world) {
        let _ = conn.outbox.try_send(Outbound::Packet(bytes.clone()));
    }
}

/// `/say <message>` — broadcast to everyone as a `say_command`-typed disguised
/// message ("[name] message"). Command arguments are unsigned, so this is a
/// `DisguisedChat` (the console/unsigned path), not a signed `PlayerChat`.
pub(super) fn cmd_say(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
    if args.is_empty() {
        return None;
    }
    let name = sender_name(world, sender)?;
    let message = text::text(args);
    let name_component = text::text(name);
    let bytes = chat::disguised_chat(&message, chat::CHAT_TYPE_SAY_COMMAND, &name_component, None);
    broadcast_all(world, bytes);
    None
}

/// `/me <action>` — broadcast an emote ("* name action") via the
/// `emote_command` chat type.
pub(super) fn cmd_me(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
    if args.is_empty() {
        return None;
    }
    let name = sender_name(world, sender)?;
    let message = text::text(args);
    let name_component = text::text(name);
    let bytes =
        chat::disguised_chat(&message, chat::CHAT_TYPE_EMOTE_COMMAND, &name_component, None);
    broadcast_all(world, bytes);
    None
}

/// `/msg <targets> <message>` (and `/tell`, `/w`) — a private message. We parse
/// the first token as a target player name (real selectors aren't modelled yet)
/// and the rest as the message. Each recipient gets a `msg_command_incoming`
/// disguised message; the sender gets a `msg_command_outgoing` echo per
/// recipient, mirroring `MsgCommand.sendMessage`.
pub(super) fn cmd_msg(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
    let (target_token, message) = args.split_once(char::is_whitespace)?;
    let message = message.trim_start();
    if target_token.is_empty() || message.is_empty() {
        return None;
    }

    let sender_display = sender_name(world, sender)?;

    // Resolve recipients by exact name (case-sensitive, like a name selector).
    let recipients: Vec<(Entity, String)> = {
        let mut q = world.query::<(Entity, &Profile)>();
        q.iter(world)
            .filter(|(_, p)| p.name == target_token)
            .map(|(e, p)| (e, p.name.clone()))
            .collect()
    };
    if recipients.is_empty() {
        // `EntityArgument.getPlayers` throws "no player found" when empty.
        return Some(text::colored(
            text::translatable("argument.entity.notfound.player", vec![]),
            "red",
        ));
    }

    let sender_component = text::text(sender_display);
    let content = text::text(message);
    for (recipient, recipient_name) in &recipients {
        let recipient_component = text::text(recipient_name.clone());
        // To the recipient: "name whispers to you: message".
        let incoming = chat::disguised_chat(
            &content,
            chat::CHAT_TYPE_MSG_COMMAND_INCOMING,
            &sender_component,
            None,
        );
        send_to(world, *recipient, incoming);
        // To the sender: "You whisper to recipient: message".
        let outgoing = chat::disguised_chat(
            &content,
            chat::CHAT_TYPE_MSG_COMMAND_OUTGOING,
            &sender_component,
            Some(&recipient_component),
        );
        send_to(world, sender, outgoing);
    }
    None
}

/// Send a framed packet to a single player's connection.
fn send_to(world: &mut World, entity: Entity, bytes: Bytes) {
    if let Some(conn) = world.get::<Conn>(entity) {
        let _ = conn.outbox.try_send(Outbound::Packet(bytes));
    }
}

// --- Player / world commands ------------------------------------------------

/// `ClientboundGameEventPacket.CHANGE_GAME_MODE` id; the param is the game type.
const GAME_EVENT_CHANGE_GAME_MODE: u8 = 3;

/// Parse a game-mode keyword (or numeric id) the way `GameType.byName` /
/// `byId` accept it. Returns the wire id 0..=3.
fn parse_game_type(s: &str) -> Option<u8> {
    Some(match s {
        "survival" | "s" | "0" => 0,
        "creative" | "c" | "1" => 1,
        "adventure" | "a" | "2" => 2,
        "spectator" | "sp" | "3" => 3,
        _ => return None,
    })
}

/// `/gamemode <mode> [<target>]` — switch a player's game mode. Without real
/// per-player game-mode state we send the client-side `CHANGE_GAME_MODE` game
/// event (so the target's client switches locally) and reply with the vanilla
/// success message. The tab-list game-mode column is not updated yet (that needs
/// a `PlayerInfoUpdate` UPDATE_GAME_MODE action).
pub(super) fn cmd_gamemode(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
    let mut it = args.split_whitespace();
    let mode_token = it.next()?;
    let Some(mode) = parse_game_type(mode_token) else {
        return Some(text::colored(
            text::translatable("argument.gamemode.invalid", vec![text::text(mode_token)]),
            "red",
        ));
    };
    let mode_component = text::translatable(&format!("gameMode.{}", game_type_name(mode)), vec![]);

    // Target: the named player, or the sender when omitted.
    let targets: Vec<Entity> = match it.next() {
        Some(name) => {
            let mut q = world.query::<(Entity, &Profile)>();
            q.iter(world)
                .filter(|(_, p)| p.name == name)
                .map(|(e, _)| e)
                .collect()
        }
        None => vec![sender],
    };
    if targets.is_empty() {
        return Some(text::colored(
            text::translatable("argument.entity.notfound.player", vec![]),
            "red",
        ));
    }

    let event = packets::game_event(GAME_EVENT_CHANGE_GAME_MODE, mode as f32);

    // Per target, mirroring `GameModeCommand.logGamemodeChange`: switch that
    // client, then emit ONE feedback line to the source for each target — the
    // `.self` key when the source is the target, else the `.other` key naming
    // that target. Non-self targets also receive a `gameMode.changed` notice.
    // Vanilla gates that notice on the SEND_COMMAND_FEEDBACK game rule; we don't
    // model it, so we use its vanilla default (true) and always send it.
    for &target in &targets {
        send_to(world, target, event.clone());
        if target == sender {
            let reply =
                text::translatable("commands.gamemode.success.self", vec![mode_component.clone()]);
            send_to(world, sender, packets::system_chat_component(&reply));
        } else {
            let target_name = sender_name(world, target).unwrap_or_default();
            let changed = text::translatable("gameMode.changed", vec![mode_component.clone()]);
            send_to(world, target, packets::system_chat_component(&changed));
            let reply = text::translatable(
                "commands.gamemode.success.other",
                vec![text::text(target_name), mode_component.clone()],
            );
            send_to(world, sender, packets::system_chat_component(&reply));
        }
    }
    None
}

/// The `GameType.getName` string for a wire id, used to build the
/// `gameMode.<name>` translation key.
fn game_type_name(id: u8) -> &'static str {
    match id {
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "survival",
    }
}
