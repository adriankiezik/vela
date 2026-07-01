//! Dynamic chunk streaming: keep each player's loaded-chunk set following them,
//! mirroring vanilla's `ChunkMap`/`PlayerChunkSender`. Runs as a system after
//! movement is applied. Also exposes the `ChunkTrackingView` membership predicate
//! used by the join path to seed a newcomer's loaded set.

use std::collections::HashMap;

use bevy_ecs::prelude::*;

use super::components::*;
use super::packets;

/// A chunk column coordinate `(cx, cz)`, used by the chunk-streaming diff.
type ChunkCoord = (i32, i32);

/// Per-column player reference count — the sim-side model of vanilla's chunk
/// ticket graph. In vanilla a column stays loaded while any `TicketType` keeps
/// its ticket level at or above the loaded threshold; the `PLAYER_LOADING`
/// tickets that pin a player's view have `timeout == 0`, so a column is loaded
/// exactly while some player tracks it. We reduce that to a plain count: how many
/// players currently have the column in their `LoadedChunks` set.
///
/// The count is incremented when a column enters a player's loaded set (movement
/// streaming, join seeding, respawn) and decremented when it leaves (movement
/// streaming, respawn, disconnect). On a decrement to zero the column is evicted
/// from the world store that same tick via [`crate::world::evict_chunk`] — an
/// incremental, reference-counted unload that replaces any periodic full-store
/// scan. This is a deliberate simplification of `ChunkMap.processUnloads`: vanilla
/// defers dropped columns through `toDrop → pendingUnloads → scheduleUnload` under
/// a per-tick time budget and keeps a slightly larger ticket radius than the view;
/// Vela evicts eagerly the same tick the last viewer leaves.
#[derive(Resource, Default)]
pub struct ChunkRefs(HashMap<ChunkCoord, u32>);

impl ChunkRefs {
    /// A player began tracking `coord`: add a reference (0 → 1 makes the column
    /// referenced; the column itself is generated lazily on first block/stream
    /// access, so there is nothing to load here).
    pub fn acquire(&mut self, coord: ChunkCoord) {
        *self.0.entry(coord).or_insert(0) += 1;
    }

    /// A player stopped tracking `coord`: drop a reference. When the *last*
    /// reference goes (count reaches zero) the column is unloaded from the world
    /// store immediately — the server-side counterpart to the `forget_chunk`
    /// `stream_chunks` sends the client. `game_time` stamps any dirty chunk the
    /// evictor must save first. A column with other viewers is merely decremented
    /// and stays resident.
    pub fn release(&mut self, coord: ChunkCoord, game_time: i64) {
        use std::collections::hash_map::Entry;
        if let Entry::Occupied(mut e) = self.0.entry(coord) {
            let n = e.get_mut();
            *n -= 1;
            if *n == 0 {
                e.remove();
                crate::world::evict_chunk(coord.0, coord.1, game_time);
            }
        }
    }

    /// The set of columns with a live reference — every column that should be
    /// resident. Used by the [`evict_untracked_chunks`] backstop as the "keep" set.
    fn referenced(&self) -> std::collections::HashSet<ChunkCoord> {
        self.0.keys().copied().collect()
    }
}

/// Dynamic chunk streaming: keep each player's loaded-chunk set following them,
/// mirroring vanilla's `ChunkMap`/`PlayerChunkSender`. Runs after
/// `broadcast_movement` so it sees the position `drain_ingress` applied this
/// tick. Per-player (each player streams to its *own* outbox), so a single
/// mutable `Query` suffices — no exclusive-`World` access needed.
///
/// Each tick, compute the player's chunk center `(floor(x)>>4, floor(z)>>4)`. If
/// it is unchanged, do nothing. Otherwise send `SetChunkCacheCenter`, then diff
/// the new view-distance square against the old one: stream `level_chunk` for
/// newly-in-range columns (nearest-first, like vanilla's distance ordering) and
/// `forget_chunk` for columns that left range, updating the `LoadedChunks` set.
pub fn stream_chunks(
    config: Res<Config>,
    time: Res<super::world_tick::WorldTime>,
    mut refs: ResMut<ChunkRefs>,
    mut players: Query<(&Pos, &Conn, &mut LoadedChunks, &mut ChunkSender, &RequestedViewDistance)>,
) {
    // `ChunkMap.getPlayerViewDistance` clamps *per player*, so the radius is computed
    // inside the loop from each player's requested distance rather than once here.
    let server_view_distance = config.0.properties.view_distance();
    let game_time = time.game_time;
    for (pos, conn, mut loaded, mut sender, requested) in players.iter_mut() {
        let radius = requested.clamped(server_view_distance);
        let center = ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4);
        if center == loaded.center {
            continue;
        }
        let (added, removed) = chunk_diff(loaded.center, center, radius);

        // Ordering-critical: a dropped SetChunkCacheCenter desyncs the client's
        // view center and strands every chunk streamed after it. On overflow flag
        // the player for a forced disconnect rather than a silent, corrupting drop.
        conn.send_reliable(packets::set_chunk_center(center.0, center.1));
        // `added` and `removed` are disjoint (leading vs trailing edge), so
        // acquiring the new before releasing the old never churns a shared column.
        // Chunks are *enqueued* (markChunkPendingToSend), not blasted: the pacer
        // in `send_queued_chunks` drains them under the batch/ack quota. `added` is
        // nearest-first (chunk_diff), preserved into the pending queue.
        for &(cx, cz) in &added {
            // Only reference-count and enqueue on a genuinely new column; the loaded
            // set is the dedup gate that keeps `pending` duplicate-free.
            if loaded.loaded.insert((cx, cz)) {
                refs.acquire((cx, cz));
                sender.mark_pending((cx, cz));
            }
        }
        for &(cx, cz) in &removed {
            loaded.loaded.remove(&(cx, cz));
            // `dropChunk`: if the column is still pending (never sent), just drop it
            // from the queue — the client never received it, so no ForgetLevelChunk.
            // Otherwise the client has it and must be told to forget it.
            if !sender.drop_pending((cx, cz)) {
                // Ordering-critical: the client believes it holds this column, so a
                // dropped ForgetLevelChunk leaks it resident client-side and desyncs
                // the view. Force-disconnect on overflow instead of dropping.
                conn.send_reliable(packets::forget_chunk(cx, cz));
            }
            // Last viewer to leave this column unloads it from the store this tick.
            refs.release((cx, cz), game_time);
        }
        loaded.center = center;
    }
}

/// Re-diff one player's loaded columns after their view distance changed
/// mid-session (a `ServerboundClientInformation` resend → vanilla
/// `ServerPlayer.updateOptions` → `ChunkMap.updateChunkTracking`). The player's
/// chunk center is unchanged, so the movement-driven `chunk_diff` — which assumes
/// a *constant* radius between two centers — cannot express this: a radius change
/// at a fixed center needs the target square recomputed at the new radius and
/// diffed against the currently-loaded set. Adds newly-in-range columns
/// (nearest-first, reference-counted and enqueued through the pacer) and forgets
/// those now out of range, exactly like [`stream_chunks`]. `loaded.center` is left
/// unchanged so subsequent movement diffs stay consistent with the resized square.
pub fn apply_view_distance_change(world: &mut World, entity: Entity) {
    let server_view_distance = world.resource::<Config>().0.properties.view_distance();
    let game_time = world.resource::<super::world_tick::WorldTime>().game_time;
    world.resource_scope(|world, mut refs: Mut<ChunkRefs>| {
        let mut q = world.query::<(
            &Pos,
            &Conn,
            &mut LoadedChunks,
            &mut ChunkSender,
            &RequestedViewDistance,
        )>();
        let Ok((pos, conn, mut loaded, mut sender, requested)) = q.get_mut(world, entity) else {
            return;
        };
        let radius = requested.clamped(server_view_distance);
        let center = ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4);
        // The target `ChunkTrackingView` square at the new radius, nearest-first to
        // match `PlayerChunkSender.collectChunksToSend`'s distance ordering. `in_view`
        // reaches `radius+1` on the axes, so the bounding box is `center ± (radius+1)`.
        let mut target: Vec<(i32, i32)> = Vec::new();
        for cx in (center.0 - radius - 1)..=(center.0 + radius + 1) {
            for cz in (center.1 - radius - 1)..=(center.1 + radius + 1) {
                if in_view(center, cx, cz, radius) {
                    target.push((cx, cz));
                }
            }
        }
        target.sort_by_key(|&(cx, cz)| {
            let dx = (cx - center.0) as i64;
            let dz = (cz - center.1) as i64;
            dx * dx + dz * dz
        });
        // Columns entering view: acquire + enqueue, but only genuinely new ones (the
        // loaded set dedups so `pending` never holds a column the player already has).
        for &(cx, cz) in &target {
            if loaded.loaded.insert((cx, cz)) {
                refs.acquire((cx, cz));
                sender.mark_pending((cx, cz));
            }
        }
        // Columns leaving view: forget only those the client actually holds (an
        // unsent-but-pending column is just dropped from the queue), then release.
        let target_set: std::collections::HashSet<(i32, i32)> = target.iter().copied().collect();
        let removed: Vec<(i32, i32)> = loaded
            .loaded
            .iter()
            .copied()
            .filter(|c| !target_set.contains(c))
            .collect();
        for (cx, cz) in removed {
            loaded.loaded.remove(&(cx, cz));
            if !sender.drop_pending((cx, cz)) {
                conn.send_reliable(packets::forget_chunk(cx, cz));
            }
            refs.release((cx, cz), game_time);
        }
    });
}

/// Drain each player's pending-chunk queue under the `PlayerChunkSender` quota,
/// one batch per tick — the server side of vanilla's chunk-batch flow control.
/// Runs every tick after `stream_chunks` (which fills the queue). Mirrors
/// `PlayerChunkSender.sendNextChunks`: when the quota gate is open, emit a
/// `ChunkBatchStart`, a quota-bounded run of `level_chunk`s (nearest-first), then
/// a `ChunkBatchFinished(n)`. The client replies `ServerboundChunkBatchReceived`,
/// which releases the next batch (see `ChunkSender::on_ack`). This is the throttle
/// that stops fast travel from flooding the client's chunk-decode backlog.
pub fn send_queued_chunks(mut players: Query<(&Conn, &mut ChunkSender)>) {
    for (conn, mut sender) in players.iter_mut() {
        let batch = sender.next_batch();
        if batch.is_empty() {
            continue;
        }
        // A chunk batch is an atomic, ordered unit: start, its level_chunks, then
        // finished(n). Dropping any frame breaks the client's batch accounting and
        // the ack it replies with, stranding chunk/light state — all reliable, so
        // an overflow mid-batch force-disconnects rather than corrupts the client.
        conn.send_reliable(packets::chunk_batch_start());
        for &(cx, cz) in &batch {
            conn.send_reliable(packets::level_chunk(cx, cz));
        }
        conn.send_reliable(packets::chunk_batch_finished(batch.len() as i32));
    }
}

/// Cadence (in ticks) of the [`evict_untracked_chunks`] backstop. This is *not*
/// the primary unload path — reference-counted eviction ([`ChunkRefs::release`]
/// → [`crate::world::evict_chunk`]) already reclaims a column the tick its last
/// viewer leaves. This slow sweep only mops up columns that were generated by a
/// bare block read/write (`with_chunk` inserts on first touch) *outside* any
/// player's loaded set, so they were never reference-counted and never released.
/// 600 ticks = 30 s: rare enough to stay off the hot path, frequent enough to
/// keep such stragglers bounded.
const BACKSTOP_INTERVAL: u64 = 600;

/// Backstop sweep for chunks that entered the store without ever being
/// reference-counted (block reads/writes outside any loaded view generate a
/// column lazily). The reference-counted [`crate::world::evict_chunk`] path
/// handles everything a player tracked; this catches only that untracked
/// remainder on the [`BACKSTOP_INTERVAL`] cadence. `keep` is the set of
/// still-referenced columns ([`ChunkRefs`]), the source of truth for what must
/// stay resident — the primary path already removed anything that fell to zero.
/// Exclusive because it reads the world clock (to stamp any dirty chunk it saves)
/// alongside `ChunkRefs`. Registered after `stream_chunks` so this tick's ref
/// updates are already applied.
pub fn evict_untracked_chunks(world: &mut World) {
    let tick = world.resource::<Tick>().0;
    if !tick.is_multiple_of(BACKSTOP_INTERVAL) {
        return;
    }
    let keep = world.resource::<ChunkRefs>().referenced();
    let game_time = world.resource::<super::world_tick::WorldTime>().game_time;
    let evicted = crate::world::evict_unused_chunks(&keep, game_time);
    // Close region files no referenced chunk maps to anymore — bounds the open-
    // file-handle cache (a no-op when persistence is disabled).
    let regions_closed = crate::world::storage::evict_regions_except(&keep);
    if evicted > 0 || regions_closed > 0 {
        tracing::debug!(evicted, regions_closed, kept = keep.len(), "backstop swept untracked chunks");
    }
}

/// Vanilla `ChunkTrackingView` membership with `includeNeighbors = true`
/// (`bufferRange = 2`, `ChunkTrackingView.isWithinDistance`): a chunk `(x, z)` is
/// tracked by a player centered at `center` with server view-distance `radius`
/// iff `max(0, |dx|-2)² + max(0, |dz|-2)² < radius²`. This reaches `radius+1` on
/// the axes and rounds the far corners off — the exact shape vanilla streams,
/// which is neither the plain `|dx|≤R ∧ |dz|≤R` square (it misses the `R+1` ring
/// and over-sends corners) nor a circle. The enclosing bounding box is
/// `center ± (radius+1)`.
pub(super) fn in_view(center: ChunkCoord, x: i32, z: i32, radius: i32) -> bool {
    let dx = ((x - center.0).abs() - 2).max(0) as i64;
    let dz = ((z - center.1).abs() - 2).max(0) as i64;
    dx * dx + dz * dz < (radius as i64) * (radius as i64)
}

/// Pure diff between two rounded `ChunkTrackingView` regions (see [`in_view`]).
/// Returns `(added, removed)`: columns tracked from `new` but not `old` (added),
/// and tracked from `old` but not `new` (removed). `added` is ordered
/// nearest-first by *squared Euclidean* chunk distance to `new`, matching
/// `PlayerChunkSender.collectChunksToSend`'s `playerPos.distanceSquared` sort.
fn chunk_diff(old: ChunkCoord, new: ChunkCoord, radius: i32) -> (Vec<ChunkCoord>, Vec<ChunkCoord>) {
    let mut added = Vec::new();
    for x in (new.0 - radius - 1)..=(new.0 + radius + 1) {
        for z in (new.1 - radius - 1)..=(new.1 + radius + 1) {
            if in_view(new, x, z, radius) && !in_view(old, x, z, radius) {
                added.push((x, z));
            }
        }
    }
    added.sort_by_key(|&(x, z)| {
        let dx = (x - new.0) as i64;
        let dz = (z - new.1) as i64;
        dx * dx + dz * dz
    });

    let mut removed = Vec::new();
    for x in (old.0 - radius - 1)..=(old.0 + radius + 1) {
        for z in (old.1 - radius - 1)..=(old.1 + radius + 1) {
            if in_view(old, x, z, radius) && !in_view(new, x, z, radius) {
                removed.push((x, z));
            }
        }
    }

    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::{chunk_diff, in_view, ChunkRefs};
    use std::collections::HashSet;

    /// A chunk shared by two viewers must stay referenced until the *last* one
    /// leaves — releasing one viewer decrements but does not drop the column;
    /// releasing the second drops it. Coordinates far from any generated column so
    /// the release-to-zero `evict_chunk` side-effect is a no-op on the world store.
    #[test]
    fn refs_hold_shared_column_until_last_viewer_leaves() {
        let c = (7_000, -7_000);
        let mut refs = ChunkRefs::default();
        refs.acquire(c);
        refs.acquire(c); // two viewers
        assert_eq!(refs.0.get(&c), Some(&2));

        refs.release(c, 0); // one leaves
        assert_eq!(refs.0.get(&c), Some(&1), "still referenced by the other viewer");

        refs.release(c, 0); // the last leaves
        assert!(!refs.0.contains_key(&c), "last viewer gone: column dereferenced");
    }

    /// Releasing a column no one references is a harmless no-op (defensive: the
    /// evictor's balance should never over-release, but it must not underflow).
    #[test]
    fn refs_release_unreferenced_is_noop() {
        let mut refs = ChunkRefs::default();
        refs.release((7_001, -7_001), 0);
        assert!(refs.0.is_empty());
    }

    /// `referenced` reports exactly the columns with a live reference — the
    /// backstop's "keep" set.
    #[test]
    fn refs_referenced_is_the_live_set() {
        let mut refs = ChunkRefs::default();
        refs.acquire((7_010, 1));
        refs.acquire((7_010, 1));
        refs.acquire((7_011, 2));
        let live = refs.referenced();
        assert_eq!(live, HashSet::from([(7_010, 1), (7_011, 2)]));
        refs.release((7_011, 2), 0);
        assert_eq!(refs.referenced(), HashSet::from([(7_010, 1)]));
    }

    /// The rounded `ChunkTrackingView` region around a center — the same
    /// predicate (`in_view`) the production diff uses, enumerated over its
    /// bounding box `center ± (radius+1)`.
    fn view_set(center: (i32, i32), radius: i32) -> HashSet<(i32, i32)> {
        let mut s = HashSet::new();
        for x in (center.0 - radius - 1)..=(center.0 + radius + 1) {
            for z in (center.1 - radius - 1)..=(center.1 + radius + 1) {
                if in_view(center, x, z, radius) {
                    s.insert((x, z));
                }
            }
        }
        s
    }

    #[test]
    fn view_reaches_axis_plus_one_and_rounds_corners() {
        // bufferRange=2: on-axis a chunk is in view out to radius+1, but the far
        // corner is rounded off. Use a realistic view distance where the rounding
        // is visible: max(0,7)²+max(0,7)² = 98 ≥ 64, so (9,9) is clipped at R=8.
        let radius = 8;
        assert!(in_view((0, 0), radius + 1, 0, radius)); // axis: reaches R+1
        assert!(!in_view((0, 0), radius + 2, 0, radius)); // but not R+2
        assert!(!in_view((0, 0), radius + 1, radius + 1, radius)); // corner clipped
    }

    #[test]
    fn diff_no_move_is_empty() {
        let (added, removed) = chunk_diff((0, 0), (0, 0), 3);
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn diff_single_step_is_symmetric_and_consistent() {
        // Moving one chunk in +x: added are exactly the columns newly in view and
        // removed exactly those that left, and (by the shape's symmetry) the two
        // sets have equal size.
        let radius = 3;
        let (added, removed) = chunk_diff((0, 0), (1, 0), radius);
        assert!(!added.is_empty());
        assert_eq!(added.len(), removed.len());
        // Added are all leading-edge (x > 0 side), removed all trailing-edge.
        assert!(added.iter().all(|&(x, _)| x > 0));
        assert!(removed.iter().all(|&(x, _)| x <= 0));
    }

    #[test]
    fn diff_matches_set_difference() {
        // The diff must equal the set-theoretic difference of the two rounded
        // view regions, for an arbitrary jump that partially overlaps.
        let old = (2, -1);
        let new = (4, 1);
        let radius = 3;
        let (added, removed) = chunk_diff(old, new, radius);

        let old_v = view_set(old, radius);
        let new_v = view_set(new, radius);
        let expect_added: HashSet<_> = new_v.difference(&old_v).copied().collect();
        let expect_removed: HashSet<_> = old_v.difference(&new_v).copied().collect();

        assert_eq!(added.iter().copied().collect::<HashSet<_>>(), expect_added);
        assert_eq!(
            removed.iter().copied().collect::<HashSet<_>>(),
            expect_removed
        );
    }

    #[test]
    fn diff_disjoint_jump_swaps_whole_regions() {
        // A jump farther than the diameter shares no chunks: the whole old region
        // is forgotten and the whole new region is loaded.
        let radius = 2;
        let (added, removed) = chunk_diff((0, 0), (100, 100), radius);
        let area = view_set((0, 0), radius).len();
        assert_eq!(added.len(), area);
        assert_eq!(removed.len(), area);
    }

    #[test]
    fn diff_added_is_nearest_first() {
        // Added chunks are ordered by squared Euclidean distance to the new
        // center, matching PlayerChunkSender's distanceSquared sort.
        let (added, _) = chunk_diff((0, 0), (5, 0), 3);
        let dist = |&(x, z): &(i32, i32)| ((x - 5) * (x - 5) + z * z) as i64;
        for w in added.windows(2) {
            assert!(dist(&w[0]) <= dist(&w[1]));
        }
    }
}
