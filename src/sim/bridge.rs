//! The contract between the async network layer and the synchronous simulation.
//!
//! This is the *only* module both `net` and `sim` import. Everything crossing
//! the boundary is a message — `net` never touches game state, `sim` never
//! touches a socket. See `docs/ARCHITECTURE.md`.

use bytes::Bytes;
use uuid::Uuid;

/// net → sim. One shared channel, its sender cloned per connection.
pub enum ToSim {
    /// A connection reached Play. Carries the per-connection outbox so the sim
    /// can talk back, and the identity established during login.
    Joined {
        id: Uuid,
        name: String,
        outbox: OutboxTx,
    },
    /// The connection's read side ended (clean EOF or decode error) — the
    /// player is gone.
    Left { id: Uuid },
    /// A decoded serverbound Play packet for the player `id`.
    Packet { id: Uuid, packet: Serverbound },
}

/// sim → net, one channel per connection. The bytes are already framed
/// (`len|id|body`), so the connection's write task is a dumb pump and the sim
/// owns all encoding.
pub type OutboxTx = tokio::sync::mpsc::Sender<Outbound>;

/// What the sim hands to a connection's write task.
#[derive(Debug)]
pub enum Outbound {
    /// A framed clientbound packet to write to the socket.
    Packet(Bytes),
    /// Tear the connection down (kicked, timed out). The write task drops the
    /// socket and aborts the paired read task.
    Close,
}

/// Decoded serverbound Play packets the simulation acts on. The `net` layer
/// owns the wire codec; the sim stays protocol-shape-agnostic. Movement carries
/// `Option`s so the four `MovePlayer*` variants collapse into one message —
/// absent fields keep their previous value.
#[derive(Debug)]
pub enum Serverbound {
    Move {
        x: Option<f64>,
        y: Option<f64>,
        z: Option<f64>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    Chat(String),
    /// A `/command` line (the client strips the leading `/`). Both the unsigned
    /// and signed serverbound variants collapse here — we run the same handlers
    /// and ignore signatures.
    ChatCommand(String),
    KeepAlive(i64),
    AcceptTeleport(i32),
    /// `ServerboundSetCarriedItemPacket` — the selected hotbar slot (0..8). The
    /// wire field is a signed short; the sim validates the range.
    SetCarriedItem { slot: i16 },
    /// `ServerboundSetCreativeModeSlotPacket` — a container slot index plus the
    /// stack to place there. The `net` layer decodes the `ItemStack` so the sim
    /// stays protocol-shape-agnostic; `None` is the empty stack.
    SetCreativeSlot {
        slot: i16,
        stack: Option<crate::inventory::ItemStack>,
    },
}
