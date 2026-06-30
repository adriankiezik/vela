//! The command system: the full vanilla command surface as one declarative
//! table, the Brigadier graph advertised to the client, and dispatch.
//!
//! Every root command a 26.2 *dedicated* server registers is listed in
//! `COMMANDS`, so the whole surface is visible in one place and a contributor
//! fills one in by flipping its [`Status`] from `Pending` to `Ready`. A command
//! whose game systems don't exist yet is still advertised, but running it
//! returns a "recognized but not yet implemented" notice naming the missing
//! subsystem rather than doing nothing — the in-code and in-game marker for what
//! remains. Implemented today: `/seed` and `/list`.
//!
//! Wire shape (`ClientboundCommandsPacket`): a flat array of nodes referenced by
//! index — a `flags` byte, a VarInt array of child indices, an optional
//! redirect, and a type-specific stub. We emit a root plus one executable
//! literal per command.
//!
//! Two known simplifications versus vanilla, both pending follow-up work:
//!   * **Argument subtrees are not serialized.** Advertising an argument node
//!     needs the `ArgumentTypeInfos` registry (parser ids + per-parser
//!     properties), which isn't built yet, so every command is a bare literal.
//!     Commands still *run* when typed with arguments (the server re-parses the
//!     raw line — e.g. `/list uuids`), they just aren't tab-completed.
//!   * **Permission levels are recorded but not enforced.** We have no op
//!     system, so every player may run every command, as on a cheats-enabled
//!     world. Vanilla's per-command level is kept in the table for fidelity and
//!     future gating. A handful of vanilla commands are also dev/IDE-gated
//!     (`raid`, `debugpath`, `chase`, …) or integrated-only (`publish`); the
//!     former are listed, the latter omitted.

use bytes::Bytes;
use bevy_ecs::prelude::*;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;
use crate::protocol::nbt::Nbt;

use super::components::{PlayerId, Profile};
use super::packets::MAX_PLAYERS;
use super::text;

/// `ClientboundCommandsPacket` — registration index in the decompiled 26.2
/// `GameProtocols` clientbound list.
const CB_PLAY_COMMANDS: i32 = 16;

// Node `flags` byte bits, from `ClientboundCommandsPacket`.
const TYPE_ROOT: u8 = 0;
const TYPE_LITERAL: u8 = 1;
const FLAG_EXECUTABLE: u8 = 4;

/// A command handler. Receives the issuing player's entity and the raw argument
/// text (everything after the command name) and returns the reply component to
/// send back to that player.
type Handler = fn(&mut World, Entity, &str) -> Nbt;

/// Whether a command is wired up.
enum Status {
    /// Implemented now; dispatch runs the handler.
    Ready(Handler),
    /// Advertised but inert until its dependency lands. The string names the
    /// missing subsystem, surfaced verbatim in the not-implemented notice.
    Pending(&'static str),
}

/// One root command.
struct Spec {
    /// The literal a player types, without the leading `/`.
    name: &'static str,
    /// Vanilla permission level (0 = everyone … 4 = owner). Recorded for fidelity
    /// and future op gating; not enforced yet.
    #[allow(dead_code)]
    permission: u8,
    status: Status,
}

impl Spec {
    const fn ready(name: &'static str, permission: u8, handler: Handler) -> Self {
        Spec { name, permission, status: Status::Ready(handler) }
    }
    const fn pending(name: &'static str, permission: u8, needs: &'static str) -> Self {
        Spec { name, permission, status: Status::Pending(needs) }
    }
}

// Dependency notes, shared by every command blocked on the same subsystem so the
// not-implemented message stays consistent. Phrased to read as "needs <note>".
const NEEDS_BLOCKS: &str = "a mutable block world";
const NEEDS_ENTITIES: &str = "the entity system";
const NEEDS_ITEMS: &str = "the item registry + inventories";
const NEEDS_INVENTORY: &str = "player inventories";
const NEEDS_PLAYERS: &str = "player state (game mode, abilities, spawn)";
const NEEDS_MESSAGE_ARG: &str = "argument-node serialization (message argument)";
const NEEDS_SCOREBOARD: &str = "the scoreboard system";
const NEEDS_WORLDGEN: &str = "world generation";
const NEEDS_TIME: &str = "server tick/time control";
const NEEDS_DATAPACK: &str = "the datapack/function engine";
const NEEDS_ADMIN: &str = "server-admin infrastructure (ban/whitelist/save)";
const NEEDS_PERMISSIONS: &str = "an op/permission system";
const NEEDS_DEBUG: &str = "debug/dev tooling (out of early scope)";

/// Every root command on a vanilla dedicated server, grouped by area. Keep this
/// the single source of truth — `commands_packet` advertises it and `run`
/// dispatches against it.
static COMMANDS: &[Spec] = &[
    // --- Implemented ---------------------------------------------------------
    Spec::ready("seed", 2, cmd_seed), // show the world seed
    Spec::ready("list", 0, cmd_list), // list online players (+ `list uuids`)

    // --- World & blocks ------------------------------------------------------
    Spec::pending("setblock", 2, NEEDS_BLOCKS),     // set a single block
    Spec::pending("fill", 2, NEEDS_BLOCKS),         // fill a region with a block
    Spec::pending("clone", 2, NEEDS_BLOCKS),        // copy a region of blocks
    Spec::pending("fillbiome", 2, NEEDS_WORLDGEN),  // set biome in a region
    Spec::pending("locate", 2, NEEDS_WORLDGEN),     // locate structure/biome/POI
    Spec::pending("place", 2, NEEDS_WORLDGEN),      // place feature/structure/jigsaw
    Spec::pending("forceload", 2, "chunk force-load tracking"), // force-load chunks
    Spec::pending("setworldspawn", 2, "a world spawn point"),   // set world spawn
    Spec::pending("worldborder", 2, "the world border"),        // manage world border
    Spec::pending("difficulty", 2, "world difficulty state"),   // query/set difficulty
    Spec::pending("gamerule", 2, "game rules"),                 // query/set game rules
    Spec::pending("particle", 2, "particle dispatch"),          // spawn particles

    // --- Entities ------------------------------------------------------------
    Spec::pending("summon", 2, NEEDS_ENTITIES),      // summon an entity
    Spec::pending("kill", 2, NEEDS_ENTITIES),        // kill entities
    Spec::pending("damage", 2, NEEDS_ENTITIES),      // apply damage to entities
    Spec::pending("teleport", 2, NEEDS_ENTITIES),    // teleport entities
    Spec::pending("tp", 2, NEEDS_ENTITIES),          // alias of teleport
    Spec::pending("ride", 2, NEEDS_ENTITIES),        // mount/dismount entities
    Spec::pending("rotate", 2, NEEDS_ENTITIES),      // rotate an entity
    Spec::pending("swing", 2, NEEDS_ENTITIES),       // make an entity swing its hand
    Spec::pending("spreadplayers", 2, NEEDS_ENTITIES), // randomly spread entities
    Spec::pending("tag", 2, "an entity tag set"),    // add/remove/list entity tags
    Spec::pending("data", 2, "block & entity NBT storage"), // get/modify NBT
    Spec::pending("attribute", 2, "entity attributes"),     // query/modify attributes
    Spec::pending("effect", 2, "mob effects"),              // give/clear status effects

    // --- Items & inventory ---------------------------------------------------
    Spec::pending("give", 2, NEEDS_ITEMS),       // give items to a player
    Spec::pending("item", 2, NEEDS_ITEMS),       // replace/modify items in slots
    Spec::pending("enchant", 2, NEEDS_ITEMS),    // enchant held/target item
    Spec::pending("loot", 2, NEEDS_ITEMS),       // generate loot-table drops
    Spec::pending("clear", 2, NEEDS_INVENTORY),  // clear items from inventory
    Spec::pending("recipe", 2, "the recipe system"), // give/take crafting recipes

    // --- Players -------------------------------------------------------------
    Spec::pending("gamemode", 2, NEEDS_PLAYERS),        // set a player's game mode
    Spec::pending("defaultgamemode", 2, NEEDS_PLAYERS), // set default game mode
    Spec::pending("spectate", 2, NEEDS_PLAYERS),        // spectate an entity
    Spec::pending("spawnpoint", 2, "a player spawn point"),    // set spawn point
    Spec::pending("experience", 2, "player experience/levels"), // query/add/set xp
    Spec::pending("xp", 2, "player experience/levels"),         // alias of experience
    Spec::pending("title", 2, "title/subtitle dispatch"),      // show title/subtitle
    Spec::pending("playsound", 2, "sound dispatch"),           // play a sound
    Spec::pending("stopsound", 2, "sound dispatch"),           // stop a sound
    Spec::pending("waypoint", 2, "locator-bar waypoints"),     // manage waypoints
    Spec::pending("bossbar", 2, "the boss-bar system"),        // manage boss bars
    Spec::pending("dialog", 2, "the dialog system"),           // show/clear dialogs
    Spec::pending("kick", 3, "argument-node serialization (player selector)"), // kick

    // --- Chat & messaging ----------------------------------------------------
    Spec::pending("say", 2, NEEDS_MESSAGE_ARG),     // broadcast a server message
    Spec::pending("me", 0, NEEDS_MESSAGE_ARG),      // broadcast an emote action
    Spec::pending("msg", 0, NEEDS_MESSAGE_ARG),     // private message a player
    Spec::pending("tell", 0, NEEDS_MESSAGE_ARG),    // alias of msg
    Spec::pending("w", 0, NEEDS_MESSAGE_ARG),       // alias of msg
    Spec::pending("teammsg", 0, NEEDS_MESSAGE_ARG), // message your team
    Spec::pending("tm", 0, NEEDS_MESSAGE_ARG),      // alias of teammsg
    Spec::pending("tellraw", 2, "argument-node serialization (component argument)"), // raw chat

    // --- Scoreboard & teams --------------------------------------------------
    Spec::pending("scoreboard", 2, NEEDS_SCOREBOARD), // objectives and scores
    Spec::pending("team", 2, NEEDS_SCOREBOARD),       // create/manage teams
    Spec::pending("trigger", 0, NEEDS_SCOREBOARD),    // activate a trigger objective

    // --- Time & progression --------------------------------------------------
    Spec::pending("time", 2, NEEDS_TIME),               // query/set/add world time
    Spec::pending("tick", 3, NEEDS_TIME),               // tick rate / freeze / step
    Spec::pending("stopwatch", 2, NEEDS_TIME),          // create/query timing stopwatches
    Spec::pending("weather", 2, "the weather system"),  // set the weather
    Spec::pending("advancement", 2, "the advancement system"), // grant/revoke advancements

    // --- Datapacks & functions -----------------------------------------------
    Spec::pending("datapack", 2, NEEDS_DATAPACK),  // enable/disable/list datapacks
    Spec::pending("function", 2, NEEDS_DATAPACK),  // run a datapack function
    Spec::pending("reload", 2, NEEDS_DATAPACK),    // reload datapacks
    Spec::pending("schedule", 2, NEEDS_DATAPACK),  // schedule a function
    Spec::pending("return", 2, "the function execution engine"), // set function return
    Spec::pending("execute", 2, "the command execution engine"), // control-flow modifier
    Spec::pending("random", 0, "RNG sequence state"),            // roll/reset RNG sequences

    // --- Meta ----------------------------------------------------------------
    Spec::pending("help", 0, "Brigadier usage formatting"), // list/show command usage
    Spec::pending("version", 2, "version reporting"),       // show server version
    Spec::pending("fetchprofile", 2, "a profile lookup service"), // fetch a profile

    // --- Server administration -----------------------------------------------
    Spec::pending("stop", 4, "graceful server shutdown"), // stop the server
    Spec::pending("transfer", 3, "cross-server transfer"), // transfer players elsewhere
    Spec::pending("op", 3, NEEDS_PERMISSIONS),    // op a player
    Spec::pending("deop", 3, NEEDS_PERMISSIONS),  // de-op a player
    Spec::pending("ban", 3, NEEDS_ADMIN),         // ban a player
    Spec::pending("ban-ip", 3, NEEDS_ADMIN),      // ban an IP address
    Spec::pending("banlist", 3, NEEDS_ADMIN),     // list bans
    Spec::pending("pardon", 3, NEEDS_ADMIN),      // unban a player
    Spec::pending("pardon-ip", 3, NEEDS_ADMIN),   // unban an IP address
    Spec::pending("whitelist", 3, NEEDS_ADMIN),   // manage the whitelist
    Spec::pending("save-all", 4, NEEDS_ADMIN),    // save the whole world
    Spec::pending("save-off", 4, NEEDS_ADMIN),    // disable automatic saving
    Spec::pending("save-on", 4, NEEDS_ADMIN),     // enable automatic saving
    Spec::pending("setidletimeout", 3, NEEDS_ADMIN), // set idle-kick timeout

    // --- Debug / dev-gated ---------------------------------------------------
    // Registered by vanilla only under dev flags (IS_RUNNING_IN_IDE /
    // DEBUG_DEV_COMMANDS / a JVM profiler / DEBUG_CHASE_COMMAND). Listed for
    // completeness; out of early scope.
    Spec::pending("debug", 3, NEEDS_DEBUG),                 // profiling traces
    Spec::pending("perf", 4, NEEDS_DEBUG),                  // perf profiling dump
    Spec::pending("jfr", 4, NEEDS_DEBUG),                   // JFR profiling
    Spec::pending("test", 2, NEEDS_DEBUG),                  // gametest framework
    Spec::pending("raid", 3, NEEDS_DEBUG),                  // debug raids
    Spec::pending("debugpath", 2, NEEDS_DEBUG),             // debug pathfinding
    Spec::pending("debugmobspawning", 2, NEEDS_DEBUG),      // debug mob spawning
    Spec::pending("debugconfig", 3, NEEDS_DEBUG),           // move players to config phase
    Spec::pending("chase", 0, NEEDS_DEBUG),                 // sync camera between clients
    Spec::pending("warden_spawn_tracker", 2, NEEDS_DEBUG),  // set warden warning level
    Spec::pending("spawn_armor_trims", 2, NEEDS_DEBUG),     // spawn all armor-trim combos
    Spec::pending("serverpack", 2, NEEDS_DEBUG),            // push/pop server resource pack
];

/// Build the framed `ClientboundCommandsPacket`: node 0 is the root, whose
/// children are nodes `1..=N`, one executable literal per command. No argument
/// subtrees or redirects yet (see the module note).
pub fn commands_packet() -> Bytes {
    let n = COMMANDS.len();
    let mut p = PacketWriter::new();
    p.write_varint((n + 1) as i32); // node count: root + one per command

    // Node 0: the root, with every command literal as a child.
    p.write_u8(TYPE_ROOT);
    p.write_varint(n as i32);
    for i in 1..=n {
        p.write_varint(i as i32);
    }

    // Nodes 1..=N: an executable, childless literal per command.
    for spec in COMMANDS {
        p.write_u8(TYPE_LITERAL | FLAG_EXECUTABLE);
        p.write_varint(0); // no children
        p.write_utf(spec.name);
    }

    p.write_varint(0); // root index
    frame(CB_PLAY_COMMANDS, &p.buf)
}

/// Run a command line (the text after the leading `/`, already stripped by the
/// client) for `sender`, returning the reply component. Unknown commands get the
/// standard red notice; recognized-but-unimplemented ones report the missing
/// subsystem.
pub fn run(world: &mut World, sender: Entity, line: &str) -> Nbt {
    let line = line.trim();
    let (name, args) = match line.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim_start()),
        None => (line, ""),
    };

    match COMMANDS.iter().find(|c| c.name == name) {
        Some(spec) => match spec.status {
            Status::Ready(handler) => handler(world, sender, args),
            Status::Pending(needs) => not_implemented(name, needs),
        },
        None => text::colored(text::translatable("command.unknown.command", vec![]), "red"),
    }
}

/// The boilerplate reply for a recognized command we haven't built yet.
fn not_implemented(name: &str, needs: &str) -> Nbt {
    text::colored(
        text::text(format!(
            "/{name} is recognized but not yet implemented (needs {needs})."
        )),
        "yellow",
    )
}

// --- Handlers ---------------------------------------------------------------

/// `/seed` — `commands.seed.success` with the click-to-copy seed value. The flat
/// world's seed is 0, matching the value sent in the play-login spawn info.
fn cmd_seed(_world: &mut World, _sender: Entity, _args: &str) -> Nbt {
    text::translatable("commands.seed.success", vec![text::copy_on_click("0")])
}

/// `/list` (and `/list uuids`) — `commands.list.players` with the online count,
/// advertised max, and the comma-joined player list. The `uuids` form appends
/// each player's UUID as `name (uuid)`, mirroring `commands.list.nameAndId`.
fn cmd_list(world: &mut World, _sender: Entity, args: &str) -> Nbt {
    let with_uuids = args.split_whitespace().next() == Some("uuids");
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
    text::translatable(
        "commands.list.players",
        vec![
            text::text(names.len().to_string()),
            text::text(MAX_PLAYERS.to_string()),
            text::text(names.join(", ")),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;

    #[test]
    fn table_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in COMMANDS {
            assert!(seen.insert(spec.name), "duplicate command name: {}", spec.name);
        }
    }

    #[test]
    fn seed_and_list_are_implemented() {
        for name in ["seed", "list"] {
            let spec = COMMANDS.iter().find(|c| c.name == name).expect("present");
            assert!(matches!(spec.status, Status::Ready(_)), "/{name} should be Ready");
        }
        // A representative blocked command stays pending.
        let give = COMMANDS.iter().find(|c| c.name == "give").unwrap();
        assert!(matches!(give.status, Status::Pending(_)));
    }

    #[test]
    fn commands_packet_decodes_to_the_table() {
        let framed = commands_packet();
        let mut r = PacketReader::new(framed);
        let _len = r.read_varint().unwrap();
        assert_eq!(r.read_varint().unwrap(), CB_PLAY_COMMANDS, "packet id");

        // Node count = root + one per command.
        assert_eq!(r.read_varint().unwrap() as usize, COMMANDS.len() + 1);

        // Root: type 0, children 1..=N in order.
        assert_eq!(r.read_u8().unwrap(), TYPE_ROOT);
        assert_eq!(r.read_varint().unwrap() as usize, COMMANDS.len());
        for i in 1..=COMMANDS.len() {
            assert_eq!(r.read_varint().unwrap() as usize, i);
        }

        // One executable literal per command, in table order.
        for spec in COMMANDS {
            assert_eq!(r.read_u8().unwrap(), TYPE_LITERAL | FLAG_EXECUTABLE);
            assert_eq!(r.read_varint().unwrap(), 0, "no children");
            assert_eq!(r.read_utf(64).unwrap(), spec.name);
        }
        assert_eq!(r.read_varint().unwrap(), 0, "root index");
    }
}
