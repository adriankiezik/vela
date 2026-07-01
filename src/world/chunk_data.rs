//! Chunk storage and lifecycle: the process-wide chunk store, each chunk's
//! generated baseline plus sparse per-cell edits, and the lazily-built/cached
//! wire `ChunkColumns`. The public block read/write API lives here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ids::BlockState;

use super::encoding::encode_blob;
use super::heightmap::compute_heightmaps;
use super::terrain::{state_at, surface_height};
use super::{states, COLUMNS, MAX_Y_EXCL, MIN_Y};

/// Compute the 256 column surface heights for chunk `(cx, cz)`, indexed
/// `lz * 16 + lx` to mirror the `(z << 4) | x` part of the cell index.
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
}

/// A chunk's mutable state: its generated baseline heights, a sparse map of
/// per-cell block-state overrides (edits), and the lazily-built/​cached wire
/// `ChunkColumns`. The wire cache is `None` until first streamed and is cleared
/// on every edit so a subsequent `level_chunk` reflects the change.
struct ChunkData {
    heights: [i32; COLUMNS],
    /// `edit_key(lx, y, lz)` → overriding block-state id (AIR included, so a
    /// broken surface block is represented explicitly). The key is a packed cell
    /// position (a bit-index, not a state); the value is the confusable id.
    edits: HashMap<u32, BlockState>,
    wire: Option<Arc<ChunkColumns>>,
}

impl ChunkData {
    fn new(cx: i32, cz: i32) -> Self {
        Self {
            heights: chunk_heights(cx, cz),
            edits: HashMap::new(),
            wire: None,
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
        state_at(y, self.heights[(lz * 16 + lx) as usize])
    }

    /// Record an edit and (only on an actual change) invalidate the wire cache,
    /// returning the previous state. Setting a cell to its generated terrain state
    /// removes any override instead of storing a redundant edit, and re-setting a
    /// cell to a value it already holds is a no-op — both keep the edit map sparse
    /// and avoid needlessly throwing away the cached wire blob.
    fn set(&mut self, lx: i32, y: i32, lz: i32, state: BlockState) -> BlockState {
        let prev = self.state(lx, y, lz);
        if let Some(key) = edit_key(lx, y, lz) {
            let generated = state_at(y, self.heights[(lz * 16 + lx) as usize]);
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
            }
        }
        prev
    }

    /// The cached wire columns, building them from heights + edits on first use.
    fn columns(&mut self) -> Arc<ChunkColumns> {
        if self.wire.is_none() {
            self.wire = Some(Arc::new(ChunkColumns {
                blob: encode_blob(&self.heights, &self.edits),
                heightmaps: compute_heightmaps(&self.heights, &self.edits),
            }));
        }
        Arc::clone(self.wire.as_ref().expect("wire just built"))
    }
}

/// The block-state at local `(lx, world_y, lz)`: an edit if present, else the
/// generated terrain state. Shared by the wire encoder and the heightmap builder,
/// which work on raw `(heights, edits)` rather than a borrowed `ChunkData`.
pub(super) fn cell_state(
    heights: &[i32; COLUMNS],
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
    state_at(world_y, heights[(lz * 16 + lx) as usize])
}

/// Encode `(lx, y, lz)` into a flat per-column-stack edit key, or `None` if `y`
/// is outside the buildable world (`MIN_Y..MAX_Y_EXCL`).
fn edit_key(lx: i32, y: i32, lz: i32) -> Option<u32> {
    if !(MIN_Y..MAX_Y_EXCL).contains(&y) {
        return None;
    }
    Some(((y - MIN_Y) as u32) * COLUMNS as u32 + (lz as u32) * 16 + lx as u32)
}

/// Process-wide store of chunks, keyed by `(cx, cz)`. Each chunk caches its wire
/// data and carries its edits. Guarded by a `Mutex` because, while the
/// simulation is single-threaded today, nothing about the signatures promises
/// that; the lock is uncontended in practice.
type ChunkStore = Mutex<HashMap<(i32, i32), ChunkData>>;

fn store() -> &'static ChunkStore {
    static STORE: OnceLock<ChunkStore> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Run `f` against chunk `(cx, cz)`'s `ChunkData`, generating it on first touch.
fn with_chunk<R>(cx: i32, cz: i32, f: impl FnOnce(&mut ChunkData) -> R) -> R {
    let mut guard = store().lock().expect("chunk store mutex poisoned");
    let chunk = guard
        .entry((cx, cz))
        .or_insert_with(|| ChunkData::new(cx, cz));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SECTION_COUNT;

    /// Build the wire columns for a chunk from its generated heights with no
    /// edits — the pre-mutable-world `generate`, kept for the encoding tests.
    fn generate(cx: i32, cz: i32) -> ChunkColumns {
        let heights = chunk_heights(cx, cz);
        let edits = HashMap::new();
        ChunkColumns {
            blob: encode_blob(&heights, &edits),
            heightmaps: compute_heightmaps(&heights, &edits),
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
        // Use a far-away column so other tests' edits can't interfere.
        let (x, y, z) = (10_000, 100, 10_000);
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
        // Break the generated surface grass at a column and confirm it reads air.
        let (wx, wz) = (10_016, 10_048);
        let h = surface_height(wx, wz);
        assert_eq!(block_state_at(wx, h, wz), states::GRASS_BLOCK);
        let prev = set_block(wx, h, wz, states::AIR);
        assert_eq!(prev, states::GRASS_BLOCK);
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
        let place_y = surface + 5; // a floating block, air between
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
    fn setting_cell_back_to_terrain_drops_override() {
        // Editing a cell to a new state records an override; setting it back to
        // the generated terrain state removes it (keeps the edit map sparse).
        let (cx, cz) = (4_242, -4_242);
        let (lx, lz) = (5, 6);
        let (wx, wz) = (cx * 16 + lx, cz * 16 + lz);
        let h = surface_height(wx, wz);
        let generated = block_state_at(wx, h, wz); // grass surface
        set_block(wx, h, wz, states::STONE);
        with_chunk(cx, cz, |c| assert_eq!(c.edits.len(), 1));
        // Back to the generated state: override is dropped, not stored.
        set_block(wx, h, wz, generated);
        with_chunk(cx, cz, |c| assert!(c.edits.is_empty()));
        assert_eq!(block_state_at(wx, h, wz), generated);
    }
}
