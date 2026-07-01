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
    /// can talk back, the identity established during login, and the client's
    /// requested view distance from the Configuration-phase `ClientInformation`
    /// (vanilla builds the `ServerPlayer` with this value via `updateOptions`,
    /// so it is known before the first chunk is sent).
    Joined {
        id: Uuid,
        name: String,
        outbox: OutboxTx,
        /// `ClientInformation.viewDistance` — the client's render distance.
        /// `getPlayerViewDistance` clamps it to `[2, serverViewDistance]`.
        view_distance: i32,
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
    /// `ServerboundChatPacket` — a chat message. The signing fields are decoded
    /// faithfully (`timestamp`/`salt`/`signature`) so the message-signing chain
    /// can be modelled; the `last_seen` acknowledgement window is decoded and
    /// dropped (we don't yet maintain a per-player last-seen tracker).
    Chat {
        message: String,
        /// Client timestamp in epoch milliseconds (`FriendlyByteBuf.readInstant`).
        timestamp: i64,
        /// The signing salt (0 when the message is unsigned).
        salt: i64,
        /// The 256-byte RSA message signature, when the client signed the message.
        signature: Option<Vec<u8>>,
    },
    /// A `/command` line (the client strips the leading `/`). Both the unsigned
    /// and signed serverbound variants collapse here — we run the same handlers
    /// and ignore signatures.
    ChatCommand(String),
    /// `ServerboundChatSessionUpdatePacket` — the client publishing its chat
    /// session (a session id plus the profile public key that signs its
    /// messages). Stored per player as the head of the message-signing chain.
    ChatSessionUpdate {
        session_id: Uuid,
        /// Key expiry in epoch milliseconds.
        expires_at: i64,
        /// The X.509-encoded RSA public key.
        public_key: Vec<u8>,
        /// Mojang's signature over the key (verified against the yggdrasil root
        /// in full secure-chat; verification is stubbed here — see `sim::chat`).
        key_signature: Vec<u8>,
    },
    /// `ServerboundCommandSuggestionPacket` — a tab-completion request for a
    /// partial command line. `id` is the transaction id echoed in the reply;
    /// `command` is the full text being completed (including the leading `/`).
    CommandSuggestion {
        id: i32,
        command: String,
    },
    KeepAlive(i64),
    AcceptTeleport(i32),
    /// `ServerboundClientInformationPacket` — the client resending its settings
    /// mid-session. Vanilla `ServerPlayer.updateOptions` copies `viewDistance`
    /// into `requestedViewDistance`, which changes the effective radius
    /// (`getPlayerViewDistance = clamp(requestedViewDistance, 2,
    /// serverViewDistance)`) and re-diffs the tracked chunks. Only the view
    /// distance is surfaced; the other options (language, chat visibility, skin
    /// customisation, main hand, …) are decoded and dropped.
    ClientInformation {
        view_distance: i32,
    },
    /// `ServerboundChunkBatchReceivedPacket` — the client acknowledges a chunk
    /// batch and reports the rate it can sustain (`desiredChunksPerTick`, a
    /// single float). Feeds the per-player `ChunkSender` throttle
    /// (`PlayerChunkSender.onChunkBatchReceivedByClient`).
    ChunkBatchReceived {
        desired_chunks_per_tick: f32,
    },
    /// `ServerboundSwingPacket` — an arm swing. `hand` is the `InteractionHand`
    /// ordinal (0 = main hand, 1 = off hand).
    Swing {
        hand: i32,
    },
    /// `ServerboundAttackPacket` — the player left-clicked to attack an entity.
    /// Carries only the target's network entity id. In 26.2 attacks were split
    /// out of `Interact`, which now covers just right-click/use interactions.
    Attack {
        entity_id: i32,
    },
    /// `ServerboundPlayerCommandPacket` — a player state action. `action` is the
    /// `Action` enum ordinal (26.2: 0 STOP_SLEEPING, 1 START_SPRINTING,
    /// 2 STOP_SPRINTING, 3 START_RIDING_JUMP, 4 STOP_RIDING_JUMP,
    /// 5 OPEN_INVENTORY, 6 START_FALL_FLYING). The leading entity id (the
    /// sender's own) and the trailing `data` argument are dropped on decode —
    /// none of the actions we currently act on use them.
    PlayerCommand {
        action: i32,
    },
    /// `ServerboundPlayerAbilitiesPacket` — the client's abilities bitset. Only
    /// the flying bit (0x02) is meaningful serverbound.
    PlayerAbilities {
        flags: u8,
    },
    /// `ServerboundPlayerInputPacket` — the player's movement-key state as a
    /// single `Input` flags byte. In 26.2 this is how the client reports crouch
    /// (the SHIFT bit); we extract only `shift` (sneaking) and ignore the rest.
    PlayerInput {
        sneaking: bool,
    },
    /// `ServerboundSetCarriedItemPacket` — the selected hotbar slot (0..8). The
    /// wire field is a signed short; the sim validates the range.
    SetCarriedItem {
        slot: i16,
    },
    /// `ServerboundSetCreativeModeSlotPacket` — a container slot index plus the
    /// stack to place there. The `net` layer decodes the `ItemStack` so the sim
    /// stays protocol-shape-agnostic; `None` is the empty stack.
    SetCreativeSlot {
        slot: i16,
        stack: Option<crate::inventory::ItemStack>,
    },
    /// `ServerboundContainerClickPacket` — a click in an open menu. We decode the
    /// resolution-relevant header (`containerId`, `stateId`, `slotNum`,
    /// `buttonNum`, the `ContainerInput` mode); the client's predicted
    /// `changedSlots`/`carriedItem` (`HashedStack`s, used only for desync
    /// detection) are left unread on the frame, since the server re-syncs the
    /// authoritative state after resolving the click.
    ContainerClick {
        container_id: i32,
        state_id: i32,
        slot: i16,
        button: i8,
        mode: i32,
    },
    /// `ServerboundContainerClosePacket` — the player closed the open screen. A
    /// single VarInt container id.
    ContainerClose {
        container_id: i32,
    },
    /// `ServerboundClientCommandPacket` — a client status request. `action` is the
    /// `Action` enum ordinal (0 PERFORM_RESPAWN, 1 REQUEST_STATS,
    /// 2 REQUEST_GAMERULE_VALUES). We act on PERFORM_RESPAWN (the click on the
    /// death screen's "Respawn" button); the others are ignored.
    ClientCommand {
        action: i32,
    },
    /// `ServerboundPlayerActionPacket` — block-dig (and item-drop) actions. The
    /// `BlockPos` long is unpacked to `(x, y, z)` by `net`. `action` is the
    /// `Action` enum ordinal (0 START_DESTROY_BLOCK, 1 ABORT_DESTROY_BLOCK,
    /// 2 STOP_DESTROY_BLOCK, …); `face` is the `Direction` 3D-data value;
    /// `sequence` is the block-change sequence to acknowledge.
    // `face` is decoded for protocol completeness but unused by the dig handler
    // (digging targets the block itself, not a neighbour).
    #[allow(dead_code)]
    PlayerAction {
        action: i32,
        x: i32,
        y: i32,
        z: i32,
        face: i32,
        sequence: i32,
    },
    /// `ServerboundUseItemOnPacket` — place/use against a block face. Carries the
    /// hit `BlockHitResult`: the targeted `(x, y, z)`, the `face` hit (Direction
    /// 3D-data value), the in-cell cursor offset, and the `inside` flag; plus the
    /// interaction `hand` ordinal and the `sequence` to acknowledge. The
    /// world-border flag is dropped on decode (unused).
    // `hand`, the cursor offset, and `inside` are decoded for protocol
    // completeness; placement only needs the target, face, and sequence so far.
    #[allow(dead_code)]
    UseItemOn {
        hand: i32,
        x: i32,
        y: i32,
        z: i32,
        face: i32,
        cursor_x: f32,
        cursor_y: f32,
        cursor_z: f32,
        inside: bool,
        sequence: i32,
    },
}
