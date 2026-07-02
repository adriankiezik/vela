//! ECS components and resources for the simulation `World`.
//!
//! A connected player is an entity carrying `PlayerId` + `Profile` + `Pos` +
//! `Conn` + `KeepAlive`. World-wide state lives in resources.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy_ecs::prelude::*;
use uuid::Uuid;

use super::bridge::{OutboxTx, ToSim};
use super::packets::JoinParams;
use crate::config::ServerConfig;

/// The player's stable identity, used to resolve incoming `ToSim` messages
/// (keyed by `Uuid`) back to this entity via `PlayerIndex`.
#[derive(Component)]
pub struct PlayerId(pub Uuid);

#[derive(Component)]
pub struct Profile {
    pub name: String,
    // Assigned at join, sent in the play-login packet, and used as the entity id
    // when this player is spawned for / moved on other players' clients.
    pub entity_id: i32,
}

/// Last-known position and orientation, updated from serverbound movement.
#[derive(Component)]
pub struct Pos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

/// Per-entity broadcast state, mirroring vanilla's `ServerEntity`: the position
/// and rotation last *sent* to tracking players. Movement packets carry deltas
/// relative to this base, and every observer shares one delta stream — so a
/// late observer's `AddEntity` is seeded from here to stay in sync.
#[derive(Component)]
pub struct Tracking {
    /// Last-sent position base (vanilla `VecDeltaCodec` base). Packet deltas are
    /// `round(cur * 4096) - round(base * 4096)`.
    pub base_x: f64,
    pub base_y: f64,
    pub base_z: f64,
    /// Last-sent packed angles (`Mth.packDegrees`: signed bytes).
    pub yaw: i8,
    pub pitch: i8,
    pub head: i8,
    /// Last-sent on-ground flag; a change forces a full position sync.
    pub on_ground: bool,
    /// Ticks since the last forced full sync (vanilla `teleportDelay`).
    pub teleport_delay: u32,
    /// Per-entity tick counter gating broadcast cadence (vanilla `tickCount`).
    pub tick_count: u32,
}

/// Player action state mirrored to other clients as entity metadata
/// (vanilla `SynchedEntityData`). `sneaking` and `sprinting` drive the shared-
/// flags byte (`Entity.DATA_SHARED_FLAGS_ID`) and the pose (`Entity.DATA_POSE`)
/// sent via `ClientboundSetEntityDataPacket`; `flying` records the abilities
/// flag (stored only — no clientbound echo yet).
///
/// Note: in 26.2 the client no longer reports crouch via
/// `ServerboundPlayerCommandPacket` (the `PRESS/RELEASE_SHIFT_KEY` actions were
/// removed in 1.21.2 — sneak now travels in `ServerboundPlayerInputPacket`), so
/// `sneaking` has no serverbound trigger yet; the metadata plumbing is in place
/// for when that packet is wired up. `sprinting` is driven by
/// `ServerboundPlayerCommandPacket`'s `START/STOP_SPRINTING`.
#[derive(Component, Default)]
pub struct Meta {
    pub sneaking: bool,
    pub sprinting: bool,
    pub flying: bool,
}

/// A player's game mode, mirroring vanilla `GameType`
/// (`net.minecraft.world.level.GameType`, MC 26.2). The discriminants match the
/// enum's ordinal / wire id exactly (`SURVIVAL=0, CREATIVE=1, ADVENTURE=2,
/// SPECTATOR=3`), which is also what `GameType.getId()` returns.
///
/// Attached lazily to a player entity on first query (seeded from the
/// server-default `gamemode` in `server.properties`); see `packet_handlers`.
/// Per-player mode replaces the previous "everyone is whatever server.properties
/// says" assumption, so e.g. block-break drops can be gated on the *breaking*
/// player's mode.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode {
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

impl GameMode {
    /// Resolve a wire/registration id to a mode. Mirrors `GameType.byId`, whose
    /// `ByIdMap.OutOfBoundsStrategy.ZERO` maps any out-of-range id to the first
    /// entry (`SURVIVAL`) rather than failing.
    pub fn from_id(id: u8) -> Self {
        match id {
            1 => GameMode::Creative,
            2 => GameMode::Adventure,
            3 => GameMode::Spectator,
            _ => GameMode::Survival,
        }
    }
}

/// The client's requested view distance, mirroring `ServerPlayer.requestedViewDistance`
/// (MC 26.2). The client advertises this as the `viewDistance` byte of
/// `ClientInformation`; the effective streaming radius is `ChunkMap.getPlayerViewDistance`
/// = `Mth.clamp(requestedViewDistance, 2, serverViewDistance)`, so a client asking for a
/// *smaller* render distance is honoured, but it can never exceed the server's.
///
/// Vanilla constructs the `ServerPlayer` *with* the `ClientInformation` received during
/// the Configuration phase (`ServerPlayer.<init>` → `updateOptions`), so this field
/// holds the real client value before any chunk is sent; its `= 2` default is never
/// actually observed by the chunk-tracking code.
///
/// Vela mirrors this: `net::connection` captures the Configuration-phase
/// `ClientInformation.viewDistance` and carries it across the bridge on
/// `ToSim::Joined`, so this component is seeded to the real client request at join.
/// A mid-session `ClientInformation` resend updates it (see
/// `packet_handlers::on_packet`), which re-diffs the player's tracked chunks via
/// `chunking::apply_view_distance_change`.
#[derive(Component, Clone, Copy)]
pub struct RequestedViewDistance(pub i32);

impl RequestedViewDistance {
    /// `ServerPlayer.requestedViewDistance`'s initial value (a not-yet-received
    /// request clamps to the floor of 2).
    pub const DEFAULT: i32 = 2;

    /// `ChunkMap.getPlayerViewDistance`: `Mth.clamp(requestedViewDistance, 2,
    /// serverViewDistance)`. Transcribed as vanilla's `Mth.clamp(int)` (`value < min ?
    /// min : min(value, max)`) rather than `i32::clamp`, which would panic — and pick a
    /// different result — were `serverViewDistance` ever below the floor of 2.
    pub fn clamped(self, server_view_distance: i32) -> i32 {
        if self.0 < 2 {
            2
        } else {
            self.0.min(server_view_distance)
        }
    }
}

/// The set of chunk columns currently streamed to a player, plus the chunk
/// center the set was last computed around. Mirrors the per-player slice of
/// vanilla's `ChunkMap`/`PlayerChunkSender` bookkeeping: the streaming system
/// only re-diffs when `center` changes, then adds/forgets columns so the loaded
/// set stays a `(2R+1)²` square around the player.
///
/// Seeded at join to exactly the square `send_join_sequence` already streamed,
/// with `center == (0, 0)`, so the streaming system sends only deltas afterwards
/// and never re-sends a chunk the join already delivered.
#[derive(Component)]
pub struct LoadedChunks {
    pub center: (i32, i32),
    pub loaded: HashSet<(i32, i32)>,
}

/// Per-player chunk-send throttle, mirroring vanilla
/// `net.minecraft.server.network.PlayerChunkSender` (MC 26.2). Every column's
/// bytes are streamed *only* through this pacer so a fast-travelling player can't
/// flood the client's decode queue (unbounded streaming balloons the client heap
/// decoding a multi-thousand-chunk backlog): columns entering view are *marked
/// pending* here, then drained a bounded number per tick under a quota that only
/// advances as the client acknowledges the batches it has received.
///
/// Field / constant parity with `PlayerChunkSender`:
/// * `MIN_CHUNKS_PER_TICK = 0.01`, `MAX_CHUNKS_PER_TICK = 64.0`,
///   `START_CHUNKS_PER_TICK = 9.0`, `MAX_UNACKNOWLEDGED_BATCHES = 10`.
/// * `desired_chunks_per_tick` (`desiredChunksPerTick`, init 9.0),
///   `batch_quota` (`batchQuota`), `unacknowledged_batches`
///   (`unacknowledgedBatches`), `max_unacknowledged_batches`
///   (`maxUnacknowledgedBatches`, init 1 — raised to 10 on the first ack, so only
///   one batch is ever outstanding until the client proves it can keep up).
///
/// Documented deviation: vanilla keeps `pendingChunks` as an unordered `LongSet`
/// and re-sorts it by distance to the player's *current* chunk position on every
/// `collectChunksToSend`. Vela instead keeps `pending` as a `Vec` already ordered
/// nearest-first at enqueue time (the `collectChunksToSend` distance sort is done
/// once by `chunk_diff` / the join-and-respawn seed) and drains from the front.
/// For a steadily-moving player the enqueue-time and drain-time orderings agree;
/// the only difference is a slightly staler ordering for chunks queued several
/// ticks before they drain — which affects the *order* of the backlog, never
/// *which* columns are sent. `memoryConnection` is always `false` here (a real
/// socket), so only the non-memory branch of `collectChunksToSend` is modelled.
#[derive(Component)]
pub struct ChunkSender {
    /// `desiredChunksPerTick` — the client's most recently requested rate.
    pub desired_chunks_per_tick: f32,
    /// `batchQuota` — accumulated fractional send budget.
    pub batch_quota: f32,
    /// `unacknowledgedBatches` — batches sent but not yet acked by the client.
    pub unacknowledged_batches: i32,
    /// `maxUnacknowledgedBatches` — the outstanding-batch cap (1 until first ack).
    pub max_unacknowledged_batches: i32,
    /// `pendingChunks` — columns queued to send, nearest-first, drained front-out.
    pub pending: Vec<(i32, i32)>,
}

impl ChunkSender {
    /// `PlayerChunkSender.MIN_CHUNKS_PER_TICK`.
    pub const MIN_CHUNKS_PER_TICK: f32 = 0.01;
    /// `PlayerChunkSender.MAX_CHUNKS_PER_TICK`.
    pub const MAX_CHUNKS_PER_TICK: f32 = 64.0;
    /// `PlayerChunkSender.START_CHUNKS_PER_TICK` — initial `desiredChunksPerTick`.
    pub const START_CHUNKS_PER_TICK: f32 = 9.0;
    /// `PlayerChunkSender.MAX_UNACKNOWLEDGED_BATCHES`.
    pub const MAX_UNACKNOWLEDGED_BATCHES: i32 = 10;

    /// A fresh sender, matching `new PlayerChunkSender(memoryConnection=false)`:
    /// quota empty, no batches outstanding, and `maxUnacknowledgedBatches == 1`.
    pub fn new() -> Self {
        Self {
            desired_chunks_per_tick: Self::START_CHUNKS_PER_TICK,
            batch_quota: 0.0,
            unacknowledged_batches: 0,
            max_unacknowledged_batches: 1,
            pending: Vec::new(),
        }
    }

    /// `markChunkPendingToSend`: queue a column for sending. Callers push in
    /// nearest-first order and only for columns *newly* entering the loaded set,
    /// so `pending` stays duplicate-free without a membership scan (the loaded set
    /// is the dedup gate — see `stream_chunks`).
    ///
    /// Also hands the column to the background prefetch pool so it is
    /// generated, lit, and encoded off the simulation thread by the time the
    /// batch pacer reaches it (see `send_queued_chunks`' readiness gate).
    pub fn mark_pending(&mut self, coord: (i32, i32)) {
        self.pending.push(coord);
        // Unit tests drive mark_pending with synthetic coordinates; don't spin
        // up worker threads generating real chunks there.
        #[cfg(not(test))]
        crate::world::prefetch([coord]);
    }

    /// `dropChunk`'s pending half: remove a column that left view before it was
    /// sent. Returns whether it was still pending — vanilla sends a
    /// `ForgetLevelChunk` only when it was *not* (the client already has it).
    pub fn drop_pending(&mut self, coord: (i32, i32)) -> bool {
        if let Some(i) = self.pending.iter().position(|&c| c == coord) {
            self.pending.remove(i);
            true
        } else {
            false
        }
    }

    /// `onChunkBatchReceivedByClient`: the client acknowledged a batch. Decrement
    /// the outstanding count (floored at 0), clamp the requested rate (NaN →
    /// `MIN_CHUNKS_PER_TICK`), reset the quota to 1 when fully caught up, and raise
    /// the outstanding-batch cap to its steady-state maximum.
    pub fn on_ack(&mut self, desired_chunks_per_tick: f32) {
        self.unacknowledged_batches -= 1;
        if self.unacknowledged_batches < 0 {
            self.unacknowledged_batches = 0;
        }
        self.desired_chunks_per_tick = if desired_chunks_per_tick.is_nan() {
            Self::MIN_CHUNKS_PER_TICK
        } else {
            desired_chunks_per_tick.clamp(Self::MIN_CHUNKS_PER_TICK, Self::MAX_CHUNKS_PER_TICK)
        };
        if self.unacknowledged_batches == 0 {
            self.batch_quota = 1.0;
        }
        self.max_unacknowledged_batches = Self::MAX_UNACKNOWLEDGED_BATCHES;
    }

    /// `sendNextChunks`' quota arithmetic and `collectChunksToSend`'s selection:
    /// return the columns to send this tick (nearest-first, drained from
    /// `pending`), or an empty vec when the gate is closed (too many
    /// unacknowledged batches, quota below one, or nothing pending). On a
    /// non-empty return the batch is already accounted — the outstanding count is
    /// incremented and the quota debited by the batch size — exactly as vanilla
    /// does after emitting the start/finished frames.
    /// Ungated form, kept for the flow-control unit tests (the live send path
    /// always gates on wire readiness).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn next_batch(&mut self) -> Vec<(i32, i32)> {
        self.next_batch_ready(usize::MAX, |_| true)
    }

    /// [`next_batch`](Self::next_batch) with a readiness gate and a size cap:
    /// within the `Mth.floor(batchQuota)` nearest candidates, ship the *ready*
    /// columns and *skip* the cold ones (leaving them in `pending`, in order),
    /// at most `limit` columns. A single still-generating near column no longer
    /// stalls the warm columns queued behind it — it is simply passed over and
    /// retried in a later batch once it warms. When the whole window is cold, or
    /// `limit` is zero, or the quota gate is shut, nothing is taken and nothing
    /// is accounted — the quota keeps accumulating and the batch goes out later.
    ///
    /// This matches `collectChunksToSend` (see [`ChunkSender`] docs for the file):
    /// vanilla takes the `Mth.floor(batchQuota)` nearest `pendingChunks`, maps
    /// each through `getChunkToSend`, and drops the nulls (columns not yet
    /// generated to FULL) — it does *not* stop at the first unready column, and
    /// the skipped columns stay pending. The window is bounded by the candidate
    /// count, so a batch may ship fewer than `floor(batchQuota)` columns when
    /// some of the nearest are still cold (vanilla does not backfill from farther
    /// out either).
    ///
    /// Documented deviations: vanilla needs no explicit readiness closure —
    /// readiness is implied by its `getChunkToSend` returning non-null only after
    /// async generation reached FULL; Vela tracks at enqueue time and checks wire
    /// readiness at send via `ready`. The `limit` cap is Vela's stand-in for TCP
    /// backpressure: vanilla's connection blocks when the socket can't drain,
    /// while Vela's bounded outbox would otherwise overflow — the caller caps the
    /// batch to the outbox headroom so the stream waits instead of tripping the
    /// fell-too-far-behind disconnect.
    pub fn next_batch_ready(
        &mut self,
        limit: usize,
        ready: impl Fn((i32, i32)) -> bool,
    ) -> Vec<(i32, i32)> {
        // `if (this.unacknowledgedBatches < this.maxUnacknowledgedBatches)`.
        if self.unacknowledged_batches >= self.max_unacknowledged_batches {
            return Vec::new();
        }
        // `batchQuota = min(batchQuota + desiredChunksPerTick, max(1.0, desired))`.
        let max_batch_size = self.desired_chunks_per_tick.max(1.0);
        self.batch_quota = (self.batch_quota + self.desired_chunks_per_tick).min(max_batch_size);
        // `if (!(this.batchQuota < 1.0F))` and `if (!this.pendingChunks.isEmpty())`.
        if self.batch_quota < 1.0 || self.pending.is_empty() {
            return Vec::new();
        }
        // `collectChunksToSend`: the candidate window is the `Mth.floor(batchQuota)`
        // nearest columns (all of `pending` if fewer). Within it, collect the ready
        // columns and skip the cold ones — vanilla's `filter(Objects::nonNull)` —
        // bounded by the caller's outbox headroom `limit`. `retain` visits
        // front-to-back, so the batch stays nearest-first and the skipped (kept)
        // columns keep their relative order.
        let window = (self.batch_quota.floor() as usize).min(self.pending.len());
        let mut batch: Vec<(i32, i32)> = Vec::new();
        let mut scanned = 0usize;
        self.pending.retain(|&c| {
            let in_window = scanned < window;
            scanned += 1;
            if in_window && batch.len() < limit && ready(c) {
                batch.push(c);
                false // sent — drop from the pending queue
            } else {
                true // cold, out of window, or over cap — stays pending
            }
        });
        // `if (!chunksToSend.isEmpty())`: account (and increment the outstanding
        // count) only when a non-empty batch actually ships.
        if batch.is_empty() {
            return Vec::new();
        }
        self.unacknowledged_batches += 1;
        self.batch_quota -= batch.len() as f32;
        batch
    }
}

impl Default for ChunkSender {
    fn default() -> Self {
        Self::new()
    }
}

/// The egress side of a player's connection — how the sim talks back. Cheap to
/// hold: a `tokio` mpsc sender plus a "disconnect requested" flag.
///
/// The outbox is a *bounded, lossy* channel (`OUTBOX_CAP`): a saturated outbox
/// means the client can't keep up. Best-effort `try_send` is fine for
/// idempotent, self-correcting packets (an entity movement delta the next
/// absolute sync will fix), but silently dropping an *ordering-critical*,
/// state-establishing packet — `set_chunk_center`, `forget_chunk`, or a chunk
/// batch frame — desyncs the client's view center irrecoverably (the "Ignoring
/// chunk since it's not in the view range" spam). Vanilla never silently drops
/// these: the connection applies TCP backpressure and, on genuine overload,
/// closes. A desynced client is worse than a disconnected one.
///
/// So [`Conn::send_reliable`] flags this connection for a clean forced
/// disconnect (a "fell too far behind" kick) on overflow instead of dropping.
/// The flag lives here — behind an `AtomicBool` so it can be raised through the
/// shared `&Conn` the streaming `Query` yields, without needing `Commands` at
/// every call site — and an exclusive system ([`super::systems::drop_lagging_players`])
/// drains it next, reusing the normal despawn/close teardown so chunk-eviction
/// refcounts stay balanced.
#[derive(Component)]
pub struct Conn {
    pub outbox: OutboxTx,
    /// Raised by [`Conn::send_reliable`] when an ordering-critical send overflows
    /// the bounded outbox; drained by the disconnect system.
    disconnect: AtomicBool,
}

impl Conn {
    /// Wrap a per-connection outbox sender, initially not flagged for disconnect.
    pub fn new(outbox: OutboxTx) -> Self {
        Self {
            outbox,
            disconnect: AtomicBool::new(false),
        }
    }

    /// Send an *ordering-critical* packet. On outbox overflow, flag this
    /// connection for a forced disconnect rather than corrupting the client with
    /// a silent drop. Returns whether it was enqueued (`false` == overflow,
    /// player now marked for disconnect). Use for `set_chunk_center`,
    /// `forget_chunk`, and the chunk-batch frames.
    pub fn send_reliable(&self, bytes: bytes::Bytes) -> bool {
        if self.outbox.try_send(super::bridge::Outbound::Packet(bytes)).is_ok() {
            true
        } else {
            self.disconnect.store(true, Ordering::Relaxed);
            false
        }
    }

    /// Whether a reliable send has overflowed and this player should be dropped.
    pub fn disconnect_requested(&self) -> bool {
        self.disconnect.load(Ordering::Relaxed)
    }

    /// Explicitly flag this connection for a forced disconnect, routing it through
    /// the same [`super::systems::drop_lagging_players`] teardown that an overflowed
    /// `send_reliable` uses. Used by the keep-alive timeout so that path saves player
    /// data and broadcasts the player-removal exactly like a clean disconnect.
    pub fn request_disconnect(&self) {
        self.disconnect.store(true, Ordering::Relaxed);
    }
}

/// Per-player keep-alive bookkeeping.
#[derive(Component)]
pub struct KeepAlive {
    pub id: i64,
    pub awaiting: bool,
    pub last_tick: u64,
}

/// The network ingress channel. Wrapped in a `Mutex` so the receiver (which is
/// `!Sync`) can live in a `Send + Sync` resource; the drain system is exclusive
/// and single-threaded, so the lock is always uncontended.
#[derive(Resource)]
pub struct Ingress(pub Mutex<tokio::sync::mpsc::Receiver<ToSim>>);

/// Monotonic tick counter (20 per second).
#[derive(Resource)]
pub struct Tick(pub u64);

/// Next entity id to hand to a joining player.
#[derive(Resource)]
pub struct NextEntityId(pub i32);

/// `Uuid` → `Entity` lookup for resolving inbound packets and disconnects.
#[derive(Resource, Default)]
pub struct PlayerIndex(pub HashMap<Uuid, Entity>);

/// Drives the run loop's exit. `stop` is set from within the simulation thread
/// (the ingress channel closed — all connections gone). `signal` is a shared
/// flag the network half sets from *another* thread to request a graceful
/// shutdown (Ctrl+C) without waiting for connections to drain; the `/stop`
/// command sets it too. The run loop stops when either is raised, then saves.
#[derive(Resource, Default)]
pub struct Control {
    pub stop: bool,
    pub signal: Arc<AtomicBool>,
}

impl Control {
    /// True when either the ingress channel closed or an external shutdown was
    /// requested (Ctrl+C / `/stop`).
    pub fn should_stop(&self) -> bool {
        self.stop || self.signal.load(Ordering::Relaxed)
    }

    /// Request a graceful shutdown from this thread (used by `/stop`).
    pub fn request_shutdown(&self) {
        self.signal.store(true, Ordering::Relaxed);
    }
}

/// The loaded server configuration, shared with the network half. Held so the
/// join sequence can be built from `server.properties` (view distance, game
/// mode, max players, …).
#[derive(Resource)]
pub struct Config(pub Arc<ServerConfig>);

impl Config {
    /// The join-packet parameters derived from `server.properties`.
    pub fn join_params(&self) -> JoinParams {
        let p = &self.0.properties;
        JoinParams {
            max_players: p.max_players(),
            view_distance: p.view_distance(),
            simulation_distance: p.simulation_distance(),
            hardcore: p.hardcore(),
            online_mode: p.online_mode(),
            game_type: p.gamemode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChunkSender, Conn};
    use bytes::Bytes;

    /// A reliable send into a full outbox flags the connection for disconnect;
    /// a reliable send with room does not. This is the core of Fix B: an
    /// ordering-critical packet must never be silently dropped — overflow becomes
    /// a clean forced disconnect instead. Exercised without a live socket by
    /// saturating a tiny bounded channel.
    #[test]
    fn reliable_send_flags_disconnect_only_on_overflow() {
        // Capacity-1 outbox; keep the receiver alive so sends fail on *fullness*,
        // not on a closed channel.
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let conn = Conn::new(tx);
        assert!(!conn.disconnect_requested(), "starts clean");

        // First reliable send fits (fills the single slot) — no disconnect.
        assert!(conn.send_reliable(Bytes::from_static(b"a")));
        assert!(!conn.disconnect_requested(), "send with room does not flag");

        // Outbox now full: the next reliable send overflows and flags the player.
        assert!(!conn.send_reliable(Bytes::from_static(b"b")));
        assert!(
            conn.disconnect_requested(),
            "overflow on an ordering-critical packet marks for disconnect"
        );
    }

    /// The flag latches: once raised it stays raised (the exclusive drop system
    /// reads it next tick), and a later successful send never clears it.
    #[test]
    fn disconnect_flag_latches() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let conn = Conn::new(tx);
        assert!(conn.send_reliable(Bytes::from_static(b"a"))); // fills slot
        assert!(!conn.send_reliable(Bytes::from_static(b"b"))); // overflow → flag
        assert!(conn.disconnect_requested());
        // Draining a slot and sending again succeeds but must not un-flag.
        rx.try_recv().expect("one queued");
        assert!(conn.send_reliable(Bytes::from_static(b"c")));
        assert!(conn.disconnect_requested(), "flag is sticky once raised");
    }

    /// A fresh sender warms up like vanilla: `maxUnacknowledgedBatches == 1`, so
    /// exactly one batch of `floor(min(0 + 9, max(1, 9))) = 9` chunks goes out,
    /// then the gate closes until the client acks. This is the flood brake.
    #[test]
    fn first_tick_sends_one_batch_then_blocks_until_ack() {
        let mut s = ChunkSender::new();
        for i in 0..100 {
            s.mark_pending((i, 0));
        }
        // Tick 1: 9 chunks (START_CHUNKS_PER_TICK), one batch outstanding.
        let b = s.next_batch();
        assert_eq!(b.len(), 9);
        assert_eq!(s.unacknowledged_batches, 1);
        assert_eq!(s.pending.len(), 91);
        // Tick 2..: gated — unacknowledged (1) is not below max (1).
        assert!(s.next_batch().is_empty());
        assert!(s.next_batch().is_empty());
        assert_eq!(s.pending.len(), 91);
    }

    /// After the first ack the cap rises to `MAX_UNACKNOWLEDGED_BATCHES = 10`, so
    /// up to ten batches can be outstanding at once before the gate closes again.
    #[test]
    fn ack_raises_cap_and_drains_over_multiple_ticks() {
        let mut s = ChunkSender::new();
        for i in 0..1000 {
            s.mark_pending((i, 0));
        }
        // Warm-up batch, then ack (client reports it can take 9/tick).
        assert_eq!(s.next_batch().len(), 9);
        s.on_ack(9.0);
        assert_eq!(s.unacknowledged_batches, 0);
        assert_eq!(s.max_unacknowledged_batches, ChunkSender::MAX_UNACKNOWLEDGED_BATCHES);
        assert_eq!(s.batch_quota, 1.0); // reset on full catch-up

        // With no acks, at most ten further batches may go out before the gate
        // shuts, each bounded by the per-tick quota.
        let mut batches = 0;
        loop {
            let b = s.next_batch();
            if b.is_empty() {
                break;
            }
            batches += 1;
            assert!(batches <= ChunkSender::MAX_UNACKNOWLEDGED_BATCHES);
        }
        assert_eq!(batches, ChunkSender::MAX_UNACKNOWLEDGED_BATCHES);
        assert_eq!(s.unacknowledged_batches, ChunkSender::MAX_UNACKNOWLEDGED_BATCHES);
        // Acking one frees exactly one more batch.
        s.on_ack(9.0);
        assert_eq!(s.unacknowledged_batches, 9);
        assert!(!s.next_batch().is_empty());
    }

    /// `dropChunk`: a column that leaves view while still pending is removed from
    /// the queue and never sent; `drop_pending` reports whether it was pending.
    #[test]
    fn drop_pending_removes_queued_chunk() {
        let mut s = ChunkSender::new();
        s.mark_pending((1, 1));
        s.mark_pending((2, 2));
        s.mark_pending((3, 3));
        assert!(s.drop_pending((2, 2)), "was queued");
        assert!(!s.drop_pending((9, 9)), "never queued");
        assert_eq!(s.pending, vec![(1, 1), (3, 3)]);
    }

    /// The readiness gate uses vanilla `collectChunksToSend` skip semantics: a
    /// cold column mid-queue is passed over (left pending, in order) while the
    /// warm columns behind it — within the quota window — still ship, and the
    /// cold one ships in a later batch once it warms.
    #[test]
    fn readiness_gate_skips_cold_and_ships_rest() {
        let mut s = ChunkSender::new();
        for i in 1..=5 {
            s.mark_pending((i, 0));
        }
        // (3,0) is cold: it is skipped, the warm columns before *and after* it
        // ship this batch, and (3,0) stays at the head of the remaining queue.
        let batch = s.next_batch_ready(usize::MAX, |c| c != (3, 0));
        assert_eq!(batch, vec![(1, 0), (2, 0), (4, 0), (5, 0)]);
        assert_eq!(s.unacknowledged_batches, 1);
        assert_eq!(s.batch_quota, 9.0 - 4.0, "quota debited by the four shipped");
        assert_eq!(s.pending, vec![(3, 0)], "the cold column stays pending, in order");
        s.on_ack(9.0);
        // Still cold: nothing ships, nothing is accounted, the queue keeps order.
        let quota_before = s.batch_quota;
        assert!(s.next_batch_ready(usize::MAX, |_| false).is_empty());
        assert_eq!(s.unacknowledged_batches, 0);
        assert!(s.batch_quota >= quota_before, "no quota debited when all cold");
        assert_eq!(s.pending, vec![(3, 0)]);
        // Warmed: the previously-skipped column ships in a later batch.
        assert_eq!(s.next_batch_ready(usize::MAX, |_| true), vec![(3, 0)]);
        assert!(s.pending.is_empty());
    }

    /// A cold column stalls only its own delivery, never the columns behind it:
    /// even the nearest column being cold does not block the rest of the window.
    #[test]
    fn readiness_gate_cold_head_does_not_block_tail() {
        let mut s = ChunkSender::new();
        for i in 1..=4 {
            s.mark_pending((i, 0));
        }
        // (1,0) — the nearest/head column — is cold; (2,0)..(4,0) ship past it.
        let batch = s.next_batch_ready(usize::MAX, |c| c != (1, 0));
        assert_eq!(batch, vec![(2, 0), (3, 0), (4, 0)]);
        assert_eq!(s.pending, vec![(1, 0)]);
        assert_eq!(s.unacknowledged_batches, 1);
    }

    /// The outbox-headroom cap bounds a batch without accounting a skipped
    /// one: zero headroom takes nothing (and debits nothing), a partial cap
    /// takes exactly that many, and the remainder ships next time.
    #[test]
    fn headroom_cap_bounds_batches() {
        let mut s = ChunkSender::new();
        for i in 1..=6 {
            s.mark_pending((i, 0));
        }
        // No headroom: the batch waits; nothing accounted.
        assert!(s.next_batch_ready(0, |_| true).is_empty());
        assert_eq!(s.unacknowledged_batches, 0);
        assert_eq!(s.pending.len(), 6);
        // Headroom for two: exactly two ship.
        assert_eq!(s.next_batch_ready(2, |_| true), vec![(1, 0), (2, 0)]);
        assert_eq!(s.unacknowledged_batches, 1);
        s.on_ack(9.0);
        // Full headroom: the rest follows in order.
        assert_eq!(
            s.next_batch_ready(usize::MAX, |_| true),
            vec![(3, 0), (4, 0), (5, 0), (6, 0)]
        );
    }

    /// `onChunkBatchReceivedByClient` decrements the outstanding count, never
    /// below zero, and clamps the client's requested rate into
    /// `[MIN, MAX]_CHUNKS_PER_TICK` (NaN maps to MIN).
    #[test]
    fn ack_decrements_and_clamps() {
        let mut s = ChunkSender::new();
        s.unacknowledged_batches = 2;
        s.on_ack(1000.0);
        assert_eq!(s.unacknowledged_batches, 1);
        assert_eq!(s.desired_chunks_per_tick, ChunkSender::MAX_CHUNKS_PER_TICK);
        s.on_ack(-5.0);
        assert_eq!(s.unacknowledged_batches, 0);
        assert_eq!(s.desired_chunks_per_tick, ChunkSender::MIN_CHUNKS_PER_TICK);
        // Never underflows below zero.
        s.on_ack(f32::NAN);
        assert_eq!(s.unacknowledged_batches, 0);
        assert_eq!(s.desired_chunks_per_tick, ChunkSender::MIN_CHUNKS_PER_TICK);
    }
}
