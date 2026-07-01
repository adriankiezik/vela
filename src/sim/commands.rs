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
//! redirect, and a type-specific stub. We emit a root plus one subtree per
//! command: a bare executable literal for most, and — for the implemented
//! commands that take arguments (`/say`, `/me`, `/msg`+`/tell`+`/w`, `/gamemode`,
//! `/time`) — real argument nodes carrying their parser id and per-parser
//! properties from the [`Parser`] table (a hand-built slice of the
//! `ArgumentTypeInfos` registry). `/tell` and `/w` are `redirect` nodes pointing
//! at `/msg`, exactly as vanilla registers them.
//!
//! Two deliberate notes versus vanilla:
//!   * **`ask_server` suggestions on player-target args.** Vanilla's
//!     `EntityArgument` computes player-name completions *client-side* (from the
//!     tab list). We instead flag the `targets`/`target` argument nodes with the
//!     `minecraft:ask_server` suggestion provider so the client routes
//!     completion through `ServerboundCommandSuggestion` and the server answers
//!     with live player names (see [`suggest`]). This is the standard ASK_SERVER
//!     mechanism many vanilla args use; only the choice to apply it here differs.
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

use super::bridge::Outbound;
use super::chat;
use super::components::{Config, Conn, PlayerId, Profile};
use super::packets;
use super::text;

/// `ClientboundCommandsPacket` — registration index in the decompiled 26.2
/// `GameProtocols` clientbound list.
const CB_PLAY_COMMANDS: i32 = 16;

// Node `flags` byte bits, from `ClientboundCommandsPacket`.
const TYPE_ROOT: u8 = 0;
const TYPE_LITERAL: u8 = 1;
const TYPE_ARGUMENT: u8 = 2;
const FLAG_EXECUTABLE: u8 = 4;
const FLAG_REDIRECT: u8 = 8;
const FLAG_CUSTOM_SUGGESTIONS: u8 = 16;

/// A command handler. Receives the issuing player's entity and the raw argument
/// text (everything after the command name). Returns `Some(component)` to reply
/// to the issuing player as system chat (vanilla `sendSuccess(_, false)`), or
/// `None` when the handler produced its own output (e.g. a chat broadcast).
type Handler = fn(&mut World, Entity, &str) -> Option<Nbt>;

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
    Spec::ready("gamemode", 2, cmd_gamemode),           // set a player's game mode
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
    Spec::ready("say", 2, cmd_say),   // broadcast a server message
    Spec::ready("me", 0, cmd_me),     // broadcast an emote action
    Spec::ready("msg", 0, cmd_msg),   // private message a player
    Spec::ready("tell", 0, cmd_msg),  // alias of msg
    Spec::ready("w", 0, cmd_msg),     // alias of msg
    Spec::pending("teammsg", 0, NEEDS_MESSAGE_ARG), // message your team
    Spec::pending("tm", 0, NEEDS_MESSAGE_ARG),      // alias of teammsg
    Spec::pending("tellraw", 2, "argument-node serialization (component argument)"), // raw chat

    // --- Scoreboard & teams --------------------------------------------------
    Spec::pending("scoreboard", 2, NEEDS_SCOREBOARD), // objectives and scores
    Spec::pending("team", 2, NEEDS_SCOREBOARD),       // create/manage teams
    Spec::pending("trigger", 0, NEEDS_SCOREBOARD),    // activate a trigger objective

    // --- Time & progression --------------------------------------------------
    Spec::ready("time", 2, cmd_time),                   // query/set/add world time
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

// --- Argument-node model (`ArgumentTypeInfos` + Brigadier node graph) --------

/// A command argument parser: its numeric id in the `COMMAND_ARGUMENT_TYPE`
/// registry (`ArgumentTypeInfos.bootstrap` registration order) plus the
/// per-parser properties it serializes. Only the parsers the implemented
/// commands need are modelled; extend as commands gain typed arguments.
#[derive(Clone, Copy)]
enum Parser {
    /// `minecraft:message` (id 20) — a free-text (optionally selector-prefixed)
    /// message. A context-free singleton: no properties.
    Message,
    /// `minecraft:entity` (id 6) — an entity/player selector. Properties are a
    /// single flags byte (`EntityArgument.Info`): `0x1` single, `0x2` players-only.
    Entity { single: bool, players_only: bool },
    /// `minecraft:gamemode` (id 42) — a game-mode keyword. Singleton: no properties.
    GameMode,
    /// `minecraft:time` (id 43) — a tick duration. Properties: the `min` int
    /// (`TimeArgument.Info`, big-endian).
    Time { min: i32 },
}

impl Parser {
    /// Registry id from `ArgumentTypeInfos.bootstrap` registration order.
    fn id(self) -> i32 {
        match self {
            Parser::Entity { .. } => 6,
            Parser::GameMode => 42,
            Parser::Message => 20,
            Parser::Time { .. } => 43,
        }
    }

    /// Serialize the parser's properties (`ArgumentTypeInfo.serializeToNetwork`).
    fn write_properties(self, p: &mut PacketWriter) {
        match self {
            Parser::Entity { single, players_only } => {
                let mut flags = 0u8;
                if single {
                    flags |= 0x1;
                }
                if players_only {
                    flags |= 0x2;
                }
                p.write_u8(flags);
            }
            Parser::Time { min } => p.write_i32(min),
            Parser::Message | Parser::GameMode => {} // singletons: no properties
        }
    }
}

/// A Brigadier command node in the graph advertised to the client.
enum NodeKind {
    Root,
    Literal(&'static str),
    Argument {
        name: &'static str,
        parser: Parser,
        /// Whether the client should ask the server for completions
        /// (`minecraft:ask_server` — sets `FLAG_CUSTOM_SUGGESTIONS`).
        ask_server: bool,
    },
}

struct Node {
    kind: NodeKind,
    executable: bool,
    redirect: Option<usize>,
    children: Vec<usize>,
}

/// A flat node graph, indexable by position; node 0 is always the root.
struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    fn new() -> Tree {
        Tree {
            nodes: vec![Node {
                kind: NodeKind::Root,
                executable: false,
                redirect: None,
                children: Vec::new(),
            }],
        }
    }

    /// Append a node and return its index.
    fn push(&mut self, kind: NodeKind, executable: bool) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node {
            kind,
            executable,
            redirect: None,
            children: Vec::new(),
        });
        index
    }

    fn add_child(&mut self, parent: usize, child: usize) {
        self.nodes[parent].children.push(child);
    }

    /// A literal `name` with a single `message`-argument child (`/say`, `/me`).
    fn message_command(&mut self, name: &'static str) -> usize {
        let lit = self.push(NodeKind::Literal(name), false);
        let arg = self.push(
            NodeKind::Argument { name: "message", parser: Parser::Message, ask_server: false },
            true,
        );
        self.add_child(lit, arg);
        lit
    }

    /// `/msg <targets> <message>` — the players-only entity selector then a
    /// message. `targets` asks the server for player-name completions.
    fn msg_command(&mut self) -> usize {
        let lit = self.push(NodeKind::Literal("msg"), false);
        let targets = self.push(
            NodeKind::Argument {
                name: "targets",
                parser: Parser::Entity { single: false, players_only: true },
                ask_server: true,
            },
            false,
        );
        let message = self.push(
            NodeKind::Argument { name: "message", parser: Parser::Message, ask_server: false },
            true,
        );
        self.add_child(targets, message);
        self.add_child(lit, targets);
        lit
    }

    /// `/gamemode <gamemode> [<target>]` — the game-mode keyword, then an
    /// optional players-only target (both executable).
    fn gamemode_command(&mut self) -> usize {
        let lit = self.push(NodeKind::Literal("gamemode"), false);
        let gamemode = self.push(
            NodeKind::Argument { name: "gamemode", parser: Parser::GameMode, ask_server: false },
            true,
        );
        let target = self.push(
            NodeKind::Argument {
                name: "target",
                parser: Parser::Entity { single: false, players_only: true },
                ask_server: true,
            },
            true,
        );
        self.add_child(gamemode, target);
        self.add_child(lit, gamemode);
        lit
    }

    /// `/time set|add <time>` and `/time query daytime|gametime|day`.
    fn time_command(&mut self) -> usize {
        let lit = self.push(NodeKind::Literal("time"), false);

        let set = self.push(NodeKind::Literal("set"), false);
        let set_time = self.push(
            NodeKind::Argument { name: "time", parser: Parser::Time { min: 0 }, ask_server: false },
            true,
        );
        self.add_child(set, set_time);

        let add = self.push(NodeKind::Literal("add"), false);
        // `/time add` accepts negatives (vanilla `TimeArgument.time(Integer.MIN_VALUE)`).
        let add_time = self.push(
            NodeKind::Argument {
                name: "time",
                parser: Parser::Time { min: i32::MIN },
                ask_server: false,
            },
            true,
        );
        self.add_child(add, add_time);

        let query = self.push(NodeKind::Literal("query"), false);
        for q in ["daytime", "gametime", "day"] {
            let node = self.push(NodeKind::Literal(q), true);
            self.add_child(query, node);
        }

        self.add_child(lit, set);
        self.add_child(lit, add);
        self.add_child(lit, query);
        lit
    }
}

/// Build the full command graph: node 0 is the root, whose children are one
/// subtree per command in table order. Most commands are bare executable
/// literals; the implemented commands that take arguments get real argument
/// subtrees, and `/tell` + `/w` are redirects to `/msg`.
fn build_tree() -> Tree {
    let mut t = Tree::new();
    let mut msg_node: Option<usize> = None;

    for spec in COMMANDS {
        let node = match spec.name {
            "say" | "me" => t.message_command(spec.name),
            "msg" => {
                let n = t.msg_command();
                msg_node = Some(n);
                n
            }
            // `/tell` and `/w` redirect to `/msg` (which precedes them in the
            // table, so its node index is known).
            "tell" | "w" => {
                let n = t.push(NodeKind::Literal(spec.name), false);
                t.nodes[n].redirect = msg_node;
                n
            }
            "gamemode" => t.gamemode_command(),
            "time" => t.time_command(),
            _ => t.push(NodeKind::Literal(spec.name), true),
        };
        t.add_child(0, node);
    }

    t
}

/// Build the framed `ClientboundCommandsPacket` from [`build_tree`]. Each entry
/// is written as `flags`, the child-index array, the redirect (when present),
/// then the type-specific stub — mirroring `ClientboundCommandsPacket.Entry.write`.
pub fn commands_packet() -> Bytes {
    let tree = build_tree();
    let mut p = PacketWriter::new();
    p.write_varint(tree.nodes.len() as i32);

    for node in &tree.nodes {
        let mut flags = match node.kind {
            NodeKind::Root => TYPE_ROOT,
            NodeKind::Literal(_) => TYPE_LITERAL,
            NodeKind::Argument { .. } => TYPE_ARGUMENT,
        };
        if node.executable {
            flags |= FLAG_EXECUTABLE;
        }
        if node.redirect.is_some() {
            flags |= FLAG_REDIRECT;
        }
        if let NodeKind::Argument { ask_server: true, .. } = node.kind {
            flags |= FLAG_CUSTOM_SUGGESTIONS;
        }
        p.write_u8(flags);

        p.write_varint(node.children.len() as i32);
        for &c in &node.children {
            p.write_varint(c as i32);
        }

        if let Some(redirect) = node.redirect {
            p.write_varint(redirect as i32);
        }

        match node.kind {
            NodeKind::Root => {}
            NodeKind::Literal(name) => p.write_utf(name),
            NodeKind::Argument { name, parser, ask_server } => {
                p.write_utf(name);
                p.write_varint(parser.id());
                parser.write_properties(&mut p);
                if ask_server {
                    p.write_identifier("minecraft:ask_server");
                }
            }
        }
    }

    p.write_varint(0); // root index
    frame(CB_PLAY_COMMANDS, &p.buf)
}

/// Run a command line (the text after the leading `/`, already stripped by the
/// client) for `sender`, returning the reply component. Unknown commands get the
/// standard red notice; recognized-but-unimplemented ones report the missing
/// subsystem.
pub fn run(world: &mut World, sender: Entity, line: &str) -> Option<Nbt> {
    let line = line.trim();
    let (name, args) = match line.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim_start()),
        None => (line, ""),
    };

    match COMMANDS.iter().find(|c| c.name == name) {
        Some(spec) => match spec.status {
            Status::Ready(handler) => handler(world, sender, args),
            Status::Pending(needs) => Some(not_implemented(name, needs)),
        },
        None => Some(text::colored(
            text::translatable("command.unknown.command", vec![]),
            "red",
        )),
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
fn cmd_seed(_world: &mut World, _sender: Entity, _args: &str) -> Option<Nbt> {
    Some(text::translatable("commands.seed.success", vec![text::copy_on_click("0")]))
}

/// `/list` (and `/list uuids`) — `commands.list.players` with the online count,
/// advertised max, and the comma-joined player list. The `uuids` form appends
/// each player's UUID as `name (uuid)`, mirroring `commands.list.nameAndId`.
fn cmd_list(world: &mut World, _sender: Entity, args: &str) -> Option<Nbt> {
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
fn cmd_say(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
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
fn cmd_me(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
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
fn cmd_msg(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
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
fn cmd_gamemode(world: &mut World, sender: Entity, args: &str) -> Option<Nbt> {
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
    for &target in &targets {
        send_to(world, target, event.clone());
    }

    // Reply: self vs other, mirroring `GameModeCommand.logGamemodeChange`.
    if targets.len() == 1 && targets[0] == sender {
        Some(text::translatable(
            "commands.gamemode.success.self",
            vec![mode_component],
        ))
    } else {
        let who = targets
            .iter()
            .filter_map(|&e| sender_name(world, e))
            .collect::<Vec<_>>()
            .join(", ");
        Some(text::translatable(
            "commands.gamemode.success.other",
            vec![text::text(who), mode_component],
        ))
    }
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

/// `/time set|add <ticks>` and `/time query daytime|gametime|day`. Drives the
/// `WorldTime` resource and broadcasts a full `SetTime` on a change, mirroring
/// `TimeCommand`. `set`/`add` accept a bare tick count or a `<n>d|s|t` unit
/// (`TimeArgument`), plus the named markers `day`/`noon`/`night`/`midnight`.
fn cmd_time(world: &mut World, _sender: Entity, args: &str) -> Option<Nbt> {
    let mut it = args.split_whitespace();
    let sub = it.next()?;
    match sub {
        "set" => {
            let ticks = parse_time_value(it.next()?)?;
            set_day_time(world, ticks);
            broadcast_time(world);
            Some(text::translatable(
                "commands.time.set",
                vec![text::text(ticks.to_string())],
            ))
        }
        "add" => {
            let delta = parse_time_value(it.next()?)?;
            let new = {
                let mut time = world.resource_mut::<super::world_tick::WorldTime>();
                time.day_time = time.day_time.wrapping_add(delta as i64);
                time.day_time
            };
            broadcast_time(world);
            Some(text::translatable(
                "commands.time.set",
                vec![text::text(new.to_string())],
            ))
        }
        "query" => {
            let time = world.resource::<super::world_tick::WorldTime>();
            let (key, value) = match it.next()? {
                "gametime" => ("commands.time.query.gametime", time.game_time),
                "day" => (
                    "commands.time.query.day",
                    (time.day_time / 24_000).rem_euclid(2_147_483_647),
                ),
                // "daytime" (and any other) reports the time of day.
                _ => ("commands.time.query", time.time_of_day()),
            };
            Some(text::translatable(key, vec![text::text(value.to_string())]))
        }
        _ => None,
    }
}

/// Parse a `/time` value: a bare tick count, a `<n>d|s|t` duration
/// (`TimeArgument`: d=24000, s=20, t=1 ticks), or a named day marker.
fn parse_time_value(s: &str) -> Option<i32> {
    match s {
        "day" => return Some(1000),
        "noon" => return Some(6000),
        "night" => return Some(13000),
        "midnight" => return Some(18000),
        _ => {}
    }
    let (num, factor) = match s.strip_suffix(['d', 's', 't']) {
        Some(rest) => (
            rest,
            match s.chars().last()? {
                'd' => 24_000.0,
                's' => 20.0,
                _ => 1.0,
            },
        ),
        None => (s, 1.0),
    };
    let value: f32 = num.parse().ok()?;
    Some((value * factor).round() as i32)
}

/// Set the overworld clock's day time (`ServerClockManager.setTotalTicks`),
/// keeping the monotonic day count so the time-of-day lands on `ticks % 24000`.
fn set_day_time(world: &mut World, ticks: i32) {
    let mut time = world.resource_mut::<super::world_tick::WorldTime>();
    // Preserve the day count; only the intra-day phase is set (vanilla setTotalTicks
    // sets the absolute total, but our day_time is the total already).
    let days = time.day_time.div_euclid(24_000);
    time.day_time = days * 24_000 + (ticks as i64).rem_euclid(24_000);
}

/// Broadcast a full `SetTime` (game time + the overworld clock, carrying its
/// rate) to every player after a `/time` change.
fn broadcast_time(world: &mut World) {
    let advance = world.resource::<super::world_tick::GameRules>().advance_time;
    let (game_time, clock) = {
        let time = world.resource::<super::world_tick::WorldTime>();
        (time.game_time, time.clock_update(advance))
    };
    broadcast_all(world, packets::set_time(game_time, &[clock]));
}

/// Answer a `ServerboundCommandSuggestionPacket`: compute completions for the
/// partial `command` line (including its leading `/`). Returns the `StringRange`
/// (`start`, `length`) the suggestions replace and the matching strings.
///
/// We complete player names for the player-target argument of `/tell`, `/msg`,
/// `/w`, and `/gamemode` (the nodes we flagged `ask_server`), and game-mode
/// keywords for `/gamemode`'s first argument — matched case-insensitively by
/// prefix, as `SharedSuggestionProvider.suggest` does.
pub fn suggest(world: &mut World, command: &str) -> (i32, i32, Vec<String>) {
    // The word under the cursor is the suffix after the last whitespace.
    let trimmed_end = command.trim_end_matches(char::is_whitespace);
    let has_trailing_space = trimmed_end.len() != command.len();
    let partial = if has_trailing_space {
        ""
    } else {
        command.rsplit(char::is_whitespace).next().unwrap_or("")
    };
    let start = command.len() - partial.len();
    let length = partial.len();

    let parts: Vec<&str> = command.split_whitespace().collect();
    let root = parts.first().copied().unwrap_or("").trim_start_matches('/');
    // Argument index 0 is the command word itself.
    let arg_index = if has_trailing_space {
        parts.len()
    } else {
        parts.len().saturating_sub(1)
    };

    let matches = |candidates: Vec<String>| -> Vec<String> {
        let needle = partial.to_lowercase();
        candidates
            .into_iter()
            .filter(|c| c.to_lowercase().starts_with(&needle))
            .collect()
    };

    let suggestions = match (root, arg_index) {
        ("tell" | "msg" | "w", 1) => matches(online_names(world)),
        ("gamemode", 1) => matches(
            ["survival", "creative", "adventure", "spectator"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        ("gamemode", 2) => matches(online_names(world)),
        _ => Vec::new(),
    };

    (start as i32, length as i32, suggestions)
}

/// The names of all online players.
fn online_names(world: &mut World) -> Vec<String> {
    let mut q = world.query::<&Profile>();
    q.iter(world).map(|p| p.name.clone()).collect()
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
    fn chat_and_messaging_commands_are_ready() {
        for name in ["say", "me", "msg", "tell", "w", "gamemode", "time"] {
            let spec = COMMANDS.iter().find(|c| c.name == name).expect("present");
            assert!(matches!(spec.status, Status::Ready(_)), "/{name} should be Ready");
        }
    }

    /// A node decoded from the framed `ClientboundCommandsPacket` — enough of the
    /// stub to assert argument-node byte layout without a full Brigadier decode.
    struct DecodedNode {
        flags: u8,
        children: Vec<usize>,
        redirect: Option<usize>,
        name: Option<String>,
        parser_id: Option<i32>,
        /// The `EntityArgument.Info` flags byte, when this is an entity argument.
        entity_flags: Option<u8>,
        /// The `TimeArgument.Info` min, when this is a time argument.
        time_min: Option<i32>,
    }

    /// Decode the whole packet into its node array. Skips per-parser properties
    /// for exactly the parsers the command table uses (message/gamemode carry
    /// none, entity a flags byte, time a min int) and the `ask_server`
    /// suggestion id — enough to walk the graph.
    fn decode_nodes(framed: Bytes) -> Vec<DecodedNode> {
        let mut r = PacketReader::new(framed);
        let _len = r.read_varint().unwrap();
        assert_eq!(r.read_varint().unwrap(), CB_PLAY_COMMANDS, "packet id");
        let count = r.read_varint().unwrap() as usize;
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            let flags = r.read_u8().unwrap();
            let nchild = r.read_varint().unwrap() as usize;
            let children = (0..nchild).map(|_| r.read_varint().unwrap() as usize).collect();
            let redirect = if flags & FLAG_REDIRECT != 0 {
                Some(r.read_varint().unwrap() as usize)
            } else {
                None
            };
            let (mut name, mut parser_id, mut entity_flags, mut time_min) =
                (None, None, None, None);
            match flags & 3 {
                1 => name = Some(r.read_utf(64).unwrap()),
                2 => {
                    name = Some(r.read_utf(64).unwrap());
                    let id = r.read_varint().unwrap();
                    parser_id = Some(id);
                    match id {
                        6 => entity_flags = Some(r.read_u8().unwrap()),
                        43 => time_min = Some(read_i32(&mut r)),
                        20 | 42 => {}
                        other => panic!("unexpected parser id {other} in test"),
                    }
                    if flags & FLAG_CUSTOM_SUGGESTIONS != 0 {
                        assert_eq!(r.read_utf(128).unwrap(), "minecraft:ask_server");
                    }
                }
                _ => {} // root
            }
            nodes.push(DecodedNode { flags, children, redirect, name, parser_id, entity_flags, time_min });
        }
        assert_eq!(r.read_varint().unwrap(), 0, "root index");
        nodes
    }

    /// Read a big-endian i32 as four bytes (the buffer has no `read_i32`).
    fn read_i32(r: &mut PacketReader) -> i32 {
        let bytes = r.read_bytes(4).unwrap();
        i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Locate a root-child literal node by its literal name.
    fn find_literal<'a>(nodes: &'a [DecodedNode], name: &str) -> (usize, &'a DecodedNode) {
        let root = &nodes[0];
        for &c in &root.children {
            if nodes[c].name.as_deref() == Some(name) {
                return (c, &nodes[c]);
            }
        }
        panic!("no root child literal {name}");
    }

    #[test]
    fn root_advertises_every_command() {
        let nodes = decode_nodes(commands_packet());
        assert_eq!(nodes[0].flags & 3, TYPE_ROOT);
        assert_eq!(nodes[0].children.len(), COMMANDS.len(), "one subtree per command");
        // A representative unimplemented command is a bare executable literal.
        let (_, give) = find_literal(&nodes, "give");
        assert_eq!(give.flags, TYPE_LITERAL | FLAG_EXECUTABLE);
        assert!(give.children.is_empty());
    }

    #[test]
    fn say_has_a_message_argument_child() {
        let nodes = decode_nodes(commands_packet());
        let (_, say) = find_literal(&nodes, "say");
        // The literal itself is not executable (it needs the message arg).
        assert_eq!(say.flags, TYPE_LITERAL);
        assert_eq!(say.children.len(), 1);
        let arg = &nodes[say.children[0]];
        assert_eq!(arg.flags & 3, TYPE_ARGUMENT);
        assert!(arg.flags & FLAG_EXECUTABLE != 0);
        assert_eq!(arg.name.as_deref(), Some("message"));
        assert_eq!(arg.parser_id, Some(20)); // minecraft:message
    }

    #[test]
    fn msg_targets_is_a_players_only_entity_with_ask_server() {
        let nodes = decode_nodes(commands_packet());
        let (_, msg) = find_literal(&nodes, "msg");
        let targets = &nodes[msg.children[0]];
        assert_eq!(targets.name.as_deref(), Some("targets"));
        assert_eq!(targets.parser_id, Some(6)); // minecraft:entity
        // EntityArgument.players(): multiple (no 0x1) + players-only (0x2).
        assert_eq!(targets.entity_flags, Some(0x2));
        assert!(targets.flags & FLAG_CUSTOM_SUGGESTIONS != 0, "asks server for names");
        // targets -> message.
        let message = &nodes[targets.children[0]];
        assert_eq!(message.parser_id, Some(20));
        assert!(message.flags & FLAG_EXECUTABLE != 0);
    }

    #[test]
    fn tell_and_w_redirect_to_msg() {
        let nodes = decode_nodes(commands_packet());
        let (msg_idx, _) = find_literal(&nodes, "msg");
        for alias in ["tell", "w"] {
            let (_, node) = find_literal(&nodes, alias);
            assert!(node.flags & FLAG_REDIRECT != 0, "/{alias} redirects");
            assert_eq!(node.redirect, Some(msg_idx), "/{alias} -> /msg");
            assert!(node.children.is_empty());
        }
    }

    #[test]
    fn time_set_and_add_carry_their_min() {
        let nodes = decode_nodes(commands_packet());
        let (_, time) = find_literal(&nodes, "time");
        // Find the `set` and `add` sub-literals.
        let mut set_min = None;
        let mut add_min = None;
        for &c in &time.children {
            let sub = &nodes[c];
            match sub.name.as_deref() {
                Some("set") => set_min = nodes[sub.children[0]].time_min,
                Some("add") => add_min = nodes[sub.children[0]].time_min,
                _ => {}
            }
        }
        assert_eq!(set_min, Some(0)); // TimeArgument.time()
        assert_eq!(add_min, Some(i32::MIN)); // TimeArgument.time(Integer.MIN_VALUE)
    }
}
