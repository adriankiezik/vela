//! The Brigadier command graph advertised to the client and its wire
//! serialization (`ClientboundCommandsPacket`). Builds a flat, index-referenced
//! node array — one subtree per command in [`COMMANDS`] order — and frames it.

use bytes::Bytes;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;

use super::COMMANDS;

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

// --- Argument-node model (`ArgumentTypeInfos` + Brigadier node graph) --------

/// A command argument parser: its numeric id in the `COMMAND_ARGUMENT_TYPE`
/// registry (`ArgumentTypeInfos.bootstrap` registration order) plus the
/// per-parser properties it serializes. Only the parsers the implemented
/// commands need are modelled; extend as commands gain typed arguments.
#[derive(Clone, Copy)]
pub(super) enum Parser {
    /// `minecraft:message` (id 20) — a free-text (optionally selector-prefixed)
    /// message. A context-free singleton: no properties.
    Message,
    /// `minecraft:entity` (id 6) — an entity/player selector. Properties are a
    /// single flags byte (`EntityArgument.Info`): `0x1` single, `0x2` players-only.
    Entity { single: bool, players_only: bool },
    /// `minecraft:gamemode` (id 42) — a game-mode keyword. Singleton: no properties.
    GameMode,
}

impl Parser {
    /// Registry id from `ArgumentTypeInfos.bootstrap` registration order.
    fn id(self) -> i32 {
        match self {
            Parser::Entity { .. } => 6,
            Parser::GameMode => 42,
            Parser::Message => 20,
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
}

/// Build the full command graph: node 0 is the root, whose children are one
/// subtree per command in table order. Most commands are bare executable
/// literals; the implemented commands that take arguments get real argument
/// subtrees, and `/tell` + `/w` are redirects to `/msg`.
fn build_tree() -> Tree {
    let mut t = Tree::new();
    // The subtree literal each `/tell` / `/w`-style redirect points at, keyed by
    // the target command name. Filled as targets are built, then resolved in a
    // second pass so redirects don't depend on table ordering.
    let mut redirect_targets: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    // Redirect nodes to wire up after every subtree exists: (node index, target name).
    let mut pending_redirects: Vec<(usize, &'static str)> = Vec::new();

    for spec in COMMANDS {
        let node = match spec.name {
            "say" | "me" => t.message_command(spec.name),
            "msg" => {
                let n = t.msg_command();
                redirect_targets.insert("msg", n);
                n
            }
            // `/tell` and `/w` are bare literals that redirect to `/msg`.
            "tell" | "w" => {
                let n = t.push(NodeKind::Literal(spec.name), false);
                pending_redirects.push((n, "msg"));
                n
            }
            "gamemode" => t.gamemode_command(),
            _ => t.push(NodeKind::Literal(spec.name), true),
        };
        t.add_child(0, node);
    }

    for (node, target) in pending_redirects {
        let dst = redirect_targets.get(target).copied();
        debug_assert!(dst.is_some(), "redirect target /{target} not built");
        t.nodes[node].redirect = dst;
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

#[cfg(test)]
mod tests {
    use super::super::{Status, COMMANDS};
    use super::*;
    use crate::protocol::buffer::PacketReader;

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
    }

    /// Decode the whole packet into its node array. Skips per-parser properties
    /// for exactly the parsers the command table uses (message/gamemode carry
    /// none, entity a flags byte) and the `ask_server` suggestion id — enough to
    /// walk the graph.
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
            let (mut name, mut parser_id, mut entity_flags) = (None, None, None);
            match flags & 3 {
                1 => name = Some(r.read_utf(64).unwrap()),
                2 => {
                    name = Some(r.read_utf(64).unwrap());
                    let id = r.read_varint().unwrap();
                    parser_id = Some(id);
                    match id {
                        6 => entity_flags = Some(r.read_u8().unwrap()),
                        20 | 42 => {}
                        other => panic!("unexpected parser id {other} in test"),
                    }
                    if flags & FLAG_CUSTOM_SUGGESTIONS != 0 {
                        assert_eq!(r.read_utf(128).unwrap(), "minecraft:ask_server");
                    }
                }
                _ => {} // root
            }
            nodes.push(DecodedNode { flags, children, redirect, name, parser_id, entity_flags });
        }
        assert_eq!(r.read_varint().unwrap(), 0, "root index");
        nodes
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
    fn time_is_pending_and_a_bare_literal() {
        // 26.2's WorldClock/ServerClockManager `/time` is out of scope; it must
        // advertise a bare executable literal, not a pre-26.2 set/add/query tree.
        let time = COMMANDS.iter().find(|c| c.name == "time").expect("present");
        assert!(matches!(time.status, Status::Pending(_)));
        let nodes = decode_nodes(commands_packet());
        let (_, node) = find_literal(&nodes, "time");
        assert_eq!(node.flags, TYPE_LITERAL | FLAG_EXECUTABLE);
        assert!(node.children.is_empty());
    }
}
