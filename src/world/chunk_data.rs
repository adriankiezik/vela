//! Chunk storage and lifecycle: the process-wide chunk store, each chunk's
//! generated baseline plus sparse per-cell edits, and the lazily-built/cached
//! wire `ChunkColumns`. The public block read/write API lives here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::warn;

use crate::ids::BlockState;

use super::encoding::encode_blob;
use super::gen::{edit_key, surface_height, GenChunk};
use super::heightmap::compute_heightmaps;
use super::{states, CELLS, COLUMNS, MAX_Y_EXCL, MIN_Y, SECTION_COUNT};

/// Compute the 256 column surface heights for chunk `(cx, cz)`, indexed
/// `lz * 16 + lx` to mirror the `(z << 4) | x` part of the cell index. A
/// convenience over [`GenChunk`] for tests and the disk round-trip that only
/// need the height field.
#[allow(dead_code)] // exercised by the chunk-store tests.
pub(super) fn chunk_heights(cx: i32, cz: i32) -> [i32; COLUMNS] {
    let mut heights = [0i32; COLUMNS];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            heights[(lz * 16 + lx) as usize] = surface_height(cx * 16 + lx, cz * 16 + lz);
        }
    }
    heights
}

/// The wire data for one chunk column: the 24-section block blob and the two
/// client-facing heightmaps. Both derive from the column's 256 surface heights
/// *plus* any per-cell edits, so they are produced together and cached; the
/// cache is invalidated whenever the chunk is edited.
pub struct ChunkColumns {
    pub blob: Vec<u8>,
    pub heightmaps: Vec<(i32, Vec<i64>)>,
    /// Converged sky/block light for the column's 26 light sections, computed
    /// together with the block blob so an edit that invalidates one re-lights the
    /// other. See [`super::light`].
    pub light: super::light::ChunkLight,
}

/// A chunk's mutable state: its generated baseline heights, a sparse map of
/// per-cell block-state overrides (edits), and the lazily-built/​cached wire
/// `ChunkColumns`. The wire cache is `None` until first streamed and is cleared
/// on every edit so a subsequent `level_chunk` reflects the change.
struct ChunkData {
    /// The deterministic generated baseline (heights, biomes, surface rule, and
    /// generated features) this chunk is built on. Player edits override it.
    gen: GenChunk,
    /// `edit_key(lx, y, lz)` → overriding block-state id (AIR included, so a
    /// broken surface block is represented explicitly). The key is a packed cell
    /// position (a bit-index, not a state); the value is the confusable id.
    edits: HashMap<u32, BlockState>,
    wire: Option<Arc<ChunkColumns>>,
    /// Set when an edit changed a cell since the last save; drives which chunks
    /// [`save_dirty_chunks`] persists. A freshly generated or just-loaded chunk
    /// is clean (already matches, or already on, disk).
    dirty: bool,
}

impl ChunkData {
    /// Generate or load chunk `(cx, cz)` on first touch: if a saved payload
    /// exists on disk it is decoded into edits (the diff of the stored blocks
    /// against freshly generated terrain), otherwise the chunk is pure generated
    /// terrain with no edits. Either way the chunk starts clean.
    fn new(cx: i32, cz: i32) -> Self {
        let gen = GenChunk::generate(cx, cz);
        if let Some(grid) = super::storage::load_chunk(cx, cz) {
            return Self::from_grid(gen, &grid);
        }
        Self {
            gen,
            edits: HashMap::new(),
            wire: None,
            dirty: false,
        }
    }

    /// Reconstruct a chunk from a loaded dense block grid (indexed
    /// `section * CELLS + (ly << 8 | lz << 4 | lx)`). Every cell that differs from
    /// the regenerated terrain baseline becomes an edit, so the in-memory chunk
    /// reproduces the saved blocks exactly. Generation is deterministic, so a
    /// chunk saved by Vela reloads to precisely its original edit set.
    fn from_grid(gen: GenChunk, grid: &[BlockState]) -> Self {
        let mut edits = HashMap::new();
        for section in 0..SECTION_COUNT {
            let base_y = MIN_Y + section * 16;
            for ly in 0..16i32 {
                let world_y = base_y + ly;
                for lz in 0..16i32 {
                    for lx in 0..16i32 {
                        let cell = (section as usize) * CELLS
                            + ((ly << 8) | (lz << 4) | lx) as usize;
                        let loaded = grid[cell];
                        let generated = gen.base_state(lx, world_y, lz);
                        if loaded != generated {
                            if let Some(key) = edit_key(lx, world_y, lz) {
                                edits.insert(key, loaded);
                            }
                        }
                    }
                }
            }
        }
        Self {
            gen,
            edits,
            wire: None,
            dirty: false,
        }
    }

    /// The block-state at local `(lx, y, lz)` — an edit if one exists, else the
    /// generated terrain state.
    fn state(&self, lx: i32, y: i32, lz: i32) -> BlockState {
        if let Some(key) = edit_key(lx, y, lz) {
            if let Some(&s) = self.edits.get(&key) {
                return s;
            }
        }
        self.gen.base_state(lx, y, lz)
    }

    /// Record an edit and (only on an actual change) invalidate the wire cache,
    /// returning the previous state. Setting a cell to its generated terrain state
    /// removes any override instead of storing a redundant edit, and re-setting a
    /// cell to a value it already holds is a no-op — both keep the edit map sparse
    /// and avoid needlessly throwing away the cached wire blob.
    fn set(&mut self, lx: i32, y: i32, lz: i32, state: BlockState) -> BlockState {
        let prev = self.state(lx, y, lz);
        if let Some(key) = edit_key(lx, y, lz) {
            let generated = self.gen.base_state(lx, y, lz);
            let changed = if state == generated {
                // Back to terrain: drop the override if one existed.
                self.edits.remove(&key).is_some()
            } else if self.edits.get(&key) == Some(&state) {
                false // already this state
            } else {
                self.edits.insert(key, state);
                true
            };
            if changed {
                self.wire = None;
                self.dirty = true;
            }
        }
        prev
    }

    /// The cached wire columns, building them from heights + edits on first use.
    fn columns(&mut self) -> Arc<ChunkColumns> {
        if self.wire.is_none() {
            self.wire = Some(Arc::new(ChunkColumns {
                blob: encode_blob(&self.gen, &self.edits),
                heightmaps: compute_heightmaps(&self.gen, &self.edits),
                light: super::light::compute_light(&self.gen, &self.edits),
            }));
        }
        Arc::clone(self.wire.as_ref().expect("wire just built"))
    }
}

/// The block-state at local `(lx, world_y, lz)`: an edit if present, else the
/// generated baseline. Shared by the wire encoder, heightmap builder, and light
/// engine, which work on raw `(gen, edits)` rather than a borrowed `ChunkData`.
pub(super) fn cell_state(
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
    lx: i32,
    world_y: i32,
    lz: i32,
) -> BlockState {
    if let Some(key) = edit_key(lx, world_y, lz) {
        if let Some(&s) = edits.get(&key) {
            return s;
        }
    }
    gen.base_state(lx, world_y, lz)
}

/// A region key `(cx >> 5, cz >> 5)` — the 32×32-column region a chunk belongs
/// to, matching the on-disk region-file granularity.
type RegionKey = (i32, i32);

fn region_key(cx: i32, cz: i32) -> RegionKey {
    (cx >> 5, cz >> 5)
}

/// The resident chunks plus a per-region resident count. The count map lets
/// eviction answer "does this region still hold any resident column?" in O(1)
/// instead of scanning every resident key, which matters on the movement hot
/// path where a boundary crossing evicts a whole edge row of columns.
///
/// Invariant: `region_counts[r]` equals the number of `columns` keys whose
/// `region_key` is `r`, and an entry is present iff that count is nonzero. Every
/// insert into / removal from `columns` must go through [`ChunkStore::insert`] /
/// [`ChunkStore::remove_column`] (or [`ChunkStore::get_or_generate`]) so the two
/// maps can never drift.
#[derive(Default)]
struct ChunkStore {
    columns: HashMap<(i32, i32), ChunkData>,
    region_counts: HashMap<RegionKey, usize>,
}

impl ChunkStore {
    /// Insert `data` at `coord`, bumping the owning region's resident count when
    /// this adds a new column (a replacement of an existing key leaves the count
    /// unchanged). Returns any displaced `ChunkData`.
    fn insert(&mut self, coord: (i32, i32), data: ChunkData) -> Option<ChunkData> {
        let replaced = self.columns.insert(coord, data);
        if replaced.is_none() {
            *self
                .region_counts
                .entry(region_key(coord.0, coord.1))
                .or_insert(0) += 1;
        }
        replaced
    }

    /// Remove `coord`, decrementing the owning region's resident count and
    /// dropping the region entry once it reaches zero. Returns the removed
    /// `ChunkData`, or `None` if it was not resident.
    fn remove_column(&mut self, coord: (i32, i32)) -> Option<ChunkData> {
        let removed = self.columns.remove(&coord);
        if removed.is_some() {
            let rk = region_key(coord.0, coord.1);
            if let Some(count) = self.region_counts.get_mut(&rk) {
                *count -= 1;
                if *count == 0 {
                    self.region_counts.remove(&rk);
                }
            }
        }
        removed
    }

    /// Get chunk `(cx, cz)`, generating and inserting it (and bumping the region
    /// count) on first touch.
    fn get_or_generate(&mut self, cx: i32, cz: i32) -> &mut ChunkData {
        if !self.columns.contains_key(&(cx, cz)) {
            self.insert((cx, cz), ChunkData::new(cx, cz));
        }
        self.columns
            .get_mut(&(cx, cz))
            .expect("column just inserted")
    }

    /// Whether the region owning `(cx, cz)` retains no resident column — an O(1)
    /// lookup against the resident-count map.
    fn region_is_empty(&self, cx: i32, cz: i32) -> bool {
        !self.region_counts.contains_key(&region_key(cx, cz))
    }
}

/// Process-wide store of chunks, keyed by `(cx, cz)`. Each chunk caches its wire
/// data and carries its edits. Guarded by a `Mutex` because, while the
/// simulation is single-threaded today, nothing about the signatures promises
/// that; the lock is uncontended in practice.
fn store() -> &'static Mutex<ChunkStore> {
    static STORE: OnceLock<Mutex<ChunkStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(ChunkStore::default()))
}

/// Run `f` against chunk `(cx, cz)`'s `ChunkData`, generating it on first touch.
fn with_chunk<R>(cx: i32, cz: i32, f: impl FnOnce(&mut ChunkData) -> R) -> R {
    let mut guard = store().lock().expect("chunk store mutex poisoned");
    let chunk = guard.get_or_generate(cx, cz);
    f(chunk)
}

/// The wire columns for chunk `(cx, cz)`, generating and caching on first
/// request and rebuilding after edits. The returned `Arc` is cheap to clone.
pub fn chunk_columns(cx: i32, cz: i32) -> Arc<ChunkColumns> {
    with_chunk(cx, cz, ChunkData::columns)
}

/// The block-state id at world `(x, y, z)` — an edit if present, else generated
/// terrain. Out-of-world `y` reads as air.
pub fn block_state_at(x: i32, y: i32, z: i32) -> BlockState {
    if !(MIN_Y..MAX_Y_EXCL).contains(&y) {
        return states::AIR;
    }
    let (cx, cz) = (x >> 4, z >> 4);
    let (lx, lz) = (x & 15, z & 15);
    with_chunk(cx, cz, |c| c.state(lx, y, lz))
}

/// Set the block-state at world `(x, y, z)`, returning the previous state id.
/// A no-op (returns air) for out-of-world `y`. Invalidates the chunk's cached
/// wire blob so a freshly-streamed `level_chunk` reflects the edit.
pub fn set_block(x: i32, y: i32, z: i32, state: BlockState) -> BlockState {
    if !(MIN_Y..MAX_Y_EXCL).contains(&y) {
        return states::AIR;
    }
    let (cx, cz) = (x >> 4, z >> 4);
    let (lx, lz) = (x & 15, z & 15);
    with_chunk(cx, cz, |c| c.set(lx, y, lz, state))
}

/// Persist every chunk edited since the last save, stamping each with
/// `game_time` as its `LastUpdate`, then flush the region files. A no-op when
/// persistence is disabled (`storage::save_chunk`/`flush` short-circuit). Called
/// periodically and on shutdown by the simulation.
pub fn save_dirty_chunks(game_time: i64) {
    if !super::storage::is_enabled() {
        return;
    }
    let mut guard = store().lock().expect("chunk store mutex poisoned");
    for ((cx, cz), chunk) in guard.columns.iter_mut() {
        if chunk.dirty {
            match super::storage::save_chunk(*cx, *cz, &chunk.gen, &chunk.edits, game_time) {
                // Only clear the dirty flag once the write succeeds; on failure
                // leave it set so the next autosave retries rather than dropping
                // the edit.
                Ok(()) => chunk.dirty = false,
                Err(e) => {
                    warn!(cx, cz, error = %e, "failed to save chunk; will retry next autosave");
                }
            }
        }
    }
    drop(guard);
    super::storage::flush();
}

/// Evict a single chunk column the moment its last player reference is dropped —
/// the primary, incremental unload path. Here the sim's per-column reference
/// count (see `sim::chunking::ChunkRefs`) hits zero and calls this, evicting the
/// column *eagerly* the same tick the last viewer leaves.
///
/// This is a deliberate simplification, not a port of vanilla's unload pipeline:
/// Vela does **not** replicate `ChunkMap.processUnloads`, which defers unloads
/// through `toDrop → pendingUnloads → scheduleUnload` gated by a per-tick time
/// budget, and which holds columns via a ticket radius larger than the view
/// distance. Vela drops the column immediately once no viewer references it.
///
/// Applies the same dirty-safety rule as [`evict_from`]: a dirty chunk is saved
/// first when persistence is enabled, and kept resident if persistence is off or
/// the save fails, so unsaved player edits are never dropped. When the chunk does
/// leave memory and no other resident chunk shares its region, the region file is
/// closed to bound open file handles. Returns whether the chunk left memory.
pub fn evict_chunk(cx: i32, cz: i32, game_time: i64) -> bool {
    let enabled = super::storage::is_enabled();
    let (evicted, saved, region_now_empty) = {
        let mut guard = store().lock().expect("chunk store mutex poisoned");
        let (evicted, saved) = evict_one_from(&mut guard, (cx, cz), enabled, game_time);
        // Whether this region retains any resident chunk, read in O(1) from the
        // per-region resident count (kept in lockstep with `columns`), so the
        // region file is closed exactly when its last resident chunk is gone.
        let region_now_empty = evicted && guard.region_is_empty(cx, cz);
        (evicted, saved, region_now_empty)
    };
    if saved {
        super::storage::flush();
    }
    if region_now_empty {
        super::storage::close_region(cx >> 5, cz >> 5);
    }
    evicted
}

/// Single-chunk eviction core, factored out of [`evict_chunk`] so it can be
/// unit-tested against a local map. Removes `coord` from `store` unless it is a
/// dirty chunk that cannot be persisted (persistence off, or the save failed), in
/// which case it is kept resident so its edits survive. Returns
/// `(evicted, saved)`: whether the chunk left the map and whether a save ran (so
/// the caller can decide to `flush`). A chunk absent from the map is a no-op.
fn evict_one_from(
    store: &mut ChunkStore,
    coord: (i32, i32),
    enabled: bool,
    game_time: i64,
) -> (bool, bool) {
    let Some(chunk) = store.columns.get(&coord) else {
        return (false, false); // never resident (e.g. never generated)
    };
    let mut saved = false;
    if chunk.dirty {
        if !enabled {
            return (false, false); // no disk to hold the edits — keep it resident
        }
        match super::storage::save_chunk(coord.0, coord.1, &chunk.gen, &chunk.edits, game_time) {
            Ok(()) => saved = true,
            Err(e) => {
                warn!(cx = coord.0, cz = coord.1, error = %e, "failed to save chunk before eviction; keeping resident");
                return (false, false); // keep so a later save retries the write
            }
        }
    }
    store.remove_column(coord);
    (true, saved)
}

/// Evict every in-memory chunk absent from `keep` (the columns still referenced
/// by some player), reclaiming the generated grid, wire cache, and light for
/// columns no player is tracking. This is the *backstop* to the incremental
/// [`evict_chunk`] path: it reclaims columns that were generated by a bare block
/// read/write (`with_chunk` inserts on first touch) outside any player's loaded
/// set, and so were never reference-counted and never released. The primary
/// mechanism is [`evict_chunk`]; this only sweeps the untracked remainder and is
/// called on a slow cadence (see `sim::chunking`), not on the hot path.
///
/// A dirty chunk is saved before eviction when persistence is enabled; if it is
/// dirty and persistence is disabled (or the save fails) the chunk is *kept*
/// resident so unsaved player edits are never silently dropped — only the vastly
/// more numerous clean, generated-only columns are freed in that case. Returns
/// the number of chunks evicted.
pub fn evict_unused_chunks(keep: &std::collections::HashSet<(i32, i32)>, game_time: i64) -> usize {
    let enabled = super::storage::is_enabled();
    let (evicted, saved_any) = {
        let mut guard = store().lock().expect("chunk store mutex poisoned");
        evict_from(&mut guard, keep, enabled, game_time)
    };
    if saved_any {
        super::storage::flush();
    }
    evicted
}

/// The eviction core, factored out of the global-store wrapper so it can be
/// unit-tested against a local map without disturbing the process-wide store.
/// Retains every chunk in `keep`; of the rest, evicts clean chunks always and
/// dirty chunks only once persisted (when `enabled`), keeping an unsaveable dirty
/// chunk resident so edits are never lost. Returns `(evicted, saved_any)` —
/// `saved_any` telling the caller whether a `flush` is warranted.
fn evict_from(
    store: &mut ChunkStore,
    keep: &std::collections::HashSet<(i32, i32)>,
    enabled: bool,
    game_time: i64,
) -> (usize, bool) {
    let mut saved_any = false;
    // Decide what to drop in one immutable pass (so persistence errors keep the
    // chunk), then remove through `remove_column` so the region counts stay in
    // lockstep — `retain` can't do the count bookkeeping mid-iteration.
    let mut to_evict: Vec<(i32, i32)> = Vec::new();
    for (&(cx, cz), chunk) in store.columns.iter() {
        if keep.contains(&(cx, cz)) {
            continue; // a player is tracking this column
        }
        if chunk.dirty {
            if !enabled {
                continue; // no disk to hold the edits — keep it resident
            }
            match super::storage::save_chunk(cx, cz, &chunk.gen, &chunk.edits, game_time) {
                Ok(()) => saved_any = true,
                Err(e) => {
                    warn!(cx, cz, error = %e, "failed to save chunk before eviction; keeping resident");
                    continue; // keep so a later autosave retries the write
                }
            }
        }
        to_evict.push((cx, cz));
    }
    let evicted = to_evict.len();
    for coord in to_evict {
        store.remove_column(coord);
    }
    (evicted, saved_any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SECTION_COUNT;

    /// Build the wire columns for a chunk from its generated heights with no
    /// edits — the pre-mutable-world `generate`, kept for the encoding tests.
    fn generate(cx: i32, cz: i32) -> ChunkColumns {
        let gen = GenChunk::generate(cx, cz);
        let edits = HashMap::new();
        ChunkColumns {
            blob: encode_blob(&gen, &edits),
            heightmaps: compute_heightmaps(&gen, &edits),
            light: crate::world::light::compute_light(&gen, &edits),
        }
    }

    #[test]
    fn chunk_columns_match_global_height() {
        // Per-chunk heights are exactly the global field at the same world xz.
        let (cx, cz) = (3, -2);
        let heights = chunk_heights(cx, cz);
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                assert_eq!(
                    heights[(lz * 16 + lx) as usize],
                    surface_height(cx * 16 + lx, cz * 16 + lz)
                );
            }
        }
    }

    #[test]
    fn heightmap_geometry() {
        let maps = generate(0, 0).heightmaps;
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].0, 1); // WORLD_SURFACE
        assert_eq!(maps[1].0, 4); // MOTION_BLOCKING
                                  // 256 columns at 9 bits, 7 per long -> 37 longs.
        assert_eq!(maps[0].1.len(), 37);
        assert_eq!(maps[1].1.len(), 37);
    }

    #[test]
    fn column_blob_is_nonempty_and_varies_by_chunk() {
        // A generated column has solid ground, so the blob exceeds the
        // all-air lower bound of 24 sections * 8 bytes.
        let a = generate(0, 0).blob;
        assert!(a.len() > (SECTION_COUNT as usize) * 8);
        // Distant chunks have different terrain, hence different bytes.
        let b = generate(50, 50).blob;
        assert_ne!(a, b);
    }

    #[test]
    fn generation_is_deterministic() {
        // Two independent generations of the same chunk match byte-for-byte.
        let a = generate(2, 5);
        let b = generate(2, 5);
        assert_eq!(a.blob, b.blob);
        assert_eq!(a.heightmaps, b.heightmaps);
    }

    #[test]
    fn chunk_columns_caches_one_instance() {
        // The cache hands back the same allocation on repeat requests.
        let a = chunk_columns(-4, 8);
        let b = chunk_columns(-4, 8);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn set_block_returns_previous_and_reads_back() {
        // Use a far-away column so other tests' edits can't interfere; y is well
        // above any generated terrain, tree, or feature so it starts as air.
        let (x, y, z) = (10_000, 250, 10_000);
        // Above the surface here is air; place stone, then read it back.
        assert_eq!(block_state_at(x, y, z), states::AIR);
        let prev = set_block(x, y, z, states::STONE);
        assert_eq!(prev, states::AIR);
        assert_eq!(block_state_at(x, y, z), states::STONE);
        // Overwrite returns the prior edit; break clears to air.
        assert_eq!(set_block(x, y, z, states::DIRT), states::STONE);
        assert_eq!(set_block(x, y, z, states::AIR), states::DIRT);
        assert_eq!(block_state_at(x, y, z), states::AIR);
    }

    #[test]
    fn breaking_surface_block_is_reflected() {
        // Break the generated surface block at a column (whatever the biome makes
        // it) and confirm the break reads back as air.
        let (wx, wz) = (10_016, 10_048);
        let h = surface_height(wx, wz);
        let top = block_state_at(wx, h, wz);
        assert_ne!(top, states::AIR, "the surface cell should be solid ground");
        let prev = set_block(wx, h, wz, states::AIR);
        assert_eq!(prev, top);
        assert_eq!(block_state_at(wx, h, wz), states::AIR);
    }

    #[test]
    fn out_of_world_edits_are_noops() {
        assert_eq!(set_block(5, MIN_Y - 1, 5, states::STONE), states::AIR);
        assert_eq!(set_block(5, MAX_Y_EXCL, 5, states::STONE), states::AIR);
        assert_eq!(block_state_at(5, MIN_Y - 1, 5), states::AIR);
    }

    #[test]
    fn edit_invalidates_wire_cache_and_rebuilds() {
        // First stream caches; an edit must invalidate so the next stream differs.
        let (cx, cz) = (321, 654);
        let before = chunk_columns(cx, cz);
        let a = chunk_columns(cx, cz);
        assert!(Arc::ptr_eq(&before, &a)); // unedited: same Arc
                                           // Place a stone pillar block well above the surface in this chunk.
        set_block(cx * 16 + 1, 200, cz * 16 + 1, states::STONE);
        let after = chunk_columns(cx, cz);
        assert!(!Arc::ptr_eq(&before, &after)); // rebuilt after the edit
        assert_ne!(before.blob, after.blob);
    }

    #[test]
    fn placing_above_surface_raises_heightmap() {
        // A block placed above the terrain surface must lift the WORLD_SURFACE
        // heightmap for that column.
        let (cx, cz) = (-321, 222);
        let (lx, lz) = (2, 3);
        let (wx, wz) = (cx * 16 + lx, cz * 16 + lz);
        let surface = surface_height(wx, wz);
        // Place well above sea level and any tree canopy so the stone is
        // unambiguously the topmost block in its column.
        let place_y = surface.max(crate::world::SURFACE_Y) + 30;
        set_block(wx, place_y, wz, states::STONE);
        let cols = chunk_columns(cx, cz);
        // Unpack the column's WORLD_SURFACE value (9-bit, 7 per long).
        let bits = 9usize;
        let per_long = 64 / bits;
        let col = (lz * 16 + lx) as usize;
        let longs = &cols.heightmaps[0].1;
        let raw = longs[col / per_long] as u64;
        let value = (raw >> ((col % per_long) * bits)) & ((1 << bits) - 1);
        assert_eq!(value as i32, place_y + 1 - MIN_Y);
    }

    #[test]
    fn eviction_frees_clean_keeps_referenced_and_keeps_unsaveable_dirty() {
        use std::collections::HashSet;
        // Operate on a *local* map via the factored-out core, so this test never
        // touches (and can't race) the process-wide store. `enabled = false` models
        // persistence being off, where a dirty chunk has no disk to hold it. The
        // lock keeps a concurrent storage test from enabling the global handle,
        // which `ChunkData::new` → `load_chunk` would otherwise observe.
        let _guard = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let kept = (10, 10);
        let clean = (11, 11);
        let dirty = (12, 12);

        let mut map = ChunkStore::default();
        map.insert(kept, ChunkData::new(kept.0, kept.1));
        map.insert(clean, ChunkData::new(clean.0, clean.1));
        let mut edited = ChunkData::new(dirty.0, dirty.1);
        edited.set(1, 200, 1, states::STONE); // an edit above the surface marks it dirty
        assert!(edited.dirty);
        map.insert(dirty, edited);

        let keep: HashSet<(i32, i32)> = [kept].into_iter().collect();
        let (evicted, saved_any) = evict_from(&mut map, &keep, false, 0);

        assert_eq!(evicted, 1, "only the clean unreferenced chunk is freed");
        assert!(!saved_any, "persistence off: nothing was written");
        assert!(map.columns.contains_key(&kept), "a referenced chunk stays resident");
        assert!(!map.columns.contains_key(&clean), "a clean unreferenced chunk is evicted");
        assert!(
            map.columns.contains_key(&dirty),
            "an unsaveable dirty chunk is kept so its edits aren't lost"
        );
        // The per-region resident count tracks exactly the surviving columns.
        assert_eq!(map.region_counts.values().sum::<usize>(), map.columns.len());
    }

    #[test]
    fn single_evict_frees_clean_but_keeps_unsaveable_dirty() {
        // The incremental (reference-count-to-zero) eviction core, tested on a
        // *local* map so it never touches the process-wide store. With persistence
        // off, a clean column evicts but a dirty one is kept resident so its edits
        // survive. Lock held for the same reason as the batch test: keep a
        // concurrent storage test from enabling the global handle that
        // `ChunkData::new` → `load_chunk` would observe.
        let _guard = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let clean = (21, 21);
        let dirty = (22, 22);

        let mut map = ChunkStore::default();
        map.insert(clean, ChunkData::new(clean.0, clean.1));
        let mut edited = ChunkData::new(dirty.0, dirty.1);
        edited.set(1, 200, 1, states::STONE); // an edit above the surface marks it dirty
        assert!(edited.dirty);
        map.insert(dirty, edited);

        // Clean column: last viewer left → evicted, nothing written.
        let (evicted, saved) = evict_one_from(&mut map, clean, false, 0);
        assert!(evicted, "a clean unreferenced column is freed");
        assert!(!saved, "persistence off: nothing was written");
        assert!(!map.columns.contains_key(&clean));

        // Dirty column with persistence off: kept resident, not evicted, not saved.
        let (evicted, saved) = evict_one_from(&mut map, dirty, false, 0);
        assert!(!evicted, "an unsaveable dirty column is kept so its edits aren't lost");
        assert!(!saved);
        assert!(map.columns.contains_key(&dirty));

        // A column that isn't resident is a no-op.
        assert_eq!(evict_one_from(&mut map, (23, 23), false, 0), (false, false));

        // The region count matches the single surviving (dirty) column, and the
        // evicted clean column's region entry was dropped to zero, not left to leak.
        assert_eq!(map.region_counts.values().sum::<usize>(), map.columns.len());
    }

    #[test]
    fn setting_cell_back_to_terrain_drops_override() {
        // Editing a cell to a new state records an override; setting it back to
        // the generated terrain state removes it (keeps the edit map sparse).
        let (cx, cz) = (4_242, -4_242);
        let (lx, lz) = (5, 6);
        let (wx, wz) = (cx * 16 + lx, cz * 16 + lz);
        // Pick a cell well above the surface (and any tree) so the generated
        // baseline is unambiguously air, distinct from the stone we place.
        let y = surface_height(wx, wz).max(crate::world::SURFACE_Y) + 40;
        let generated = block_state_at(wx, y, wz); // air above the terrain
        set_block(wx, y, wz, states::STONE);
        with_chunk(cx, cz, |c| assert_eq!(c.edits.len(), 1));
        // Back to the generated state: override is dropped, not stored.
        set_block(wx, y, wz, generated);
        with_chunk(cx, cz, |c| assert!(c.edits.is_empty()));
        assert_eq!(block_state_at(wx, y, wz), generated);
    }
}
