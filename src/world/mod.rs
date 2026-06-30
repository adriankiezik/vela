//! World data representation — the chunk-section wire encoding, the
//! bit-packing primitive underneath it, and a small noise-based terrain
//! generator.
//!
//! A chunk column is 24 stacked sections of 16×16×16 cells rising from the
//! world floor (`MIN_Y` = -64). Each section serializes exactly as vanilla's
//! `LevelChunkSection`: a non-air block count, a fluid count, a block-state
//! `PalettedContainer`, then a biome `PalettedContainer`. We emit the wire bytes
//! for a *static* world directly rather than modelling the full mutable
//! container — enough to stream a generated world a chunk at a time.
//!
//! Terrain is a hand-written value-noise heightmap (a couple of fbm octaves),
//! deterministic in world coordinates so it is continuous across chunk
//! boundaries. This is intentionally *not* a port of vanilla's
//! `NoiseRouter`/`DensityFunction` stack — just enough rolling terrain to stand
//! on.
//!
//! Reference: decompiled `LevelChunkSection`, `PalettedContainer`, `Strategy`,
//! and `Heightmap` (MC 26.2). The numeric block-state ids come from the server's
//! own block registration order (`Blocks.java`) / `--reports` block dump
//! (observable output), not copied source.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::protocol::buffer::PacketWriter;

mod block_item;
pub use block_item::block_state_for_item;

/// World floor. Sections stack upward from here; the overworld is 384 blocks
/// tall, so 24 sections of 16.
pub const MIN_Y: i32 = -64;
/// Sections per column (384 / 16).
pub const SECTION_COUNT: i32 = 24;
/// Cells per 16×16×16 section.
const CELLS: usize = 16 * 16 * 16;
/// Columns per chunk (16×16), one heightmap entry each.
const COLUMNS: usize = 16 * 16;

/// Reference surface height. Terrain is centred on this so a player spawned at
/// y=64 lands on the ground near the origin.
pub const SURFACE_Y: i32 = 63;

/// The air block-state id (palette 0) — the "empty" cell and the result of a
/// break. Public so the simulation can place/clear blocks without reaching into
/// the private `states` table.
pub const AIR_STATE: u32 = 0;

/// Global block-state palette ids — the default state of each block, taken from
/// the server's block registration order in `Blocks.java` (AIR registered first
/// → state 0, STONE second → state 1) and the generated `reports/blocks.json`
/// for 26.2.
mod states {
    pub const AIR: u32 = 0;
    /// STONE is the second block registered (single state) → state id 1.
    pub const STONE: u32 = 1;
    pub const GRASS_BLOCK: u32 = 9;
    pub const DIRT: u32 = 10;
    pub const BEDROCK: u32 = 85;
}

/// The biome a section's biome `PalettedContainer` reports, as a *network*
/// registry index into the biome registry we sync in `crate::registries`. Index
/// 39 is `minecraft:plains` in that list — a sensible match for green grassy
/// terrain (index 0 would be `badlands`, which tints grass orange). The whole
/// world reports this single biome for now.
const PLAINS_BIOME: u32 = 39;

// ---------------------------------------------------------------------------
// Terrain generation
// ---------------------------------------------------------------------------

/// Fixed world seed. We do not thread the server.properties seed through here —
/// a constant keeps generation deterministic and reproducible.
const SEED: u32 = 0x5EED_C0DE;

/// Lowest possible surface height the generator will emit.
const HEIGHT_MIN: i32 = 56;
/// Highest possible surface height the generator will emit.
const HEIGHT_MAX: i32 = 96;
/// Horizontal feature size: world blocks per unit of the base noise lattice.
/// Larger = broader, gentler hills.
const TERRAIN_SCALE: f64 = 96.0;
/// Vertical amplitude of the terrain around `SURFACE_Y`, before clamping.
const TERRAIN_AMPLITUDE: f64 = 18.0;
/// Number of fbm octaves summed for the heightfield.
const OCTAVES: u32 = 3;

/// 32-bit integer hash of a lattice point. A cheap finalizer (xorshift-multiply
/// avalanche) — enough decorrelation for value noise, deterministic everywhere.
fn hash2(x: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed;
    h ^= (x as u32).wrapping_mul(0x9E37_79B1);
    h = h.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h ^= (z as u32).wrapping_mul(0xC2B2_AE3D);
    h = h.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 13;
    h
}

/// Pseudo-random value in `[-1, 1]` at an integer lattice point.
fn lattice(x: i32, z: i32, seed: u32) -> f64 {
    let h = hash2(x, z, seed);
    (h as f64 / u32::MAX as f64) * 2.0 - 1.0
}

/// Smoothstep (`3t² − 2t³`) for C¹-continuous interpolation between lattice
/// points — avoids the visible creases of plain linear value noise.
fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Bilinear value noise sampled at `(x, z)` in lattice space, in `[-1, 1]`.
fn value_noise(x: f64, z: f64, seed: u32) -> f64 {
    let x0 = x.floor() as i32;
    let z0 = z.floor() as i32;
    let sx = smoothstep(x - x0 as f64);
    let sz = smoothstep(z - z0 as f64);

    let v00 = lattice(x0, z0, seed);
    let v10 = lattice(x0 + 1, z0, seed);
    let v01 = lattice(x0, z0 + 1, seed);
    let v11 = lattice(x0 + 1, z0 + 1, seed);

    let top = lerp(v00, v10, sx);
    let bottom = lerp(v01, v11, sx);
    lerp(top, bottom, sz)
}

/// Fractional Brownian motion: `OCTAVES` octaves of value noise, each doubling
/// frequency and halving amplitude. Normalised back to roughly `[-1, 1]`.
fn fbm(x: f64, z: f64) -> f64 {
    let mut sum = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut total_amplitude = 0.0;
    for octave in 0..OCTAVES {
        // Per-octave seed offset so octaves don't share the same lattice.
        sum += amplitude * value_noise(x * frequency, z * frequency, SEED ^ (octave + 1));
        total_amplitude += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / total_amplitude
}

/// Surface height (the y of the topmost solid/grass block) for a world column.
/// Deterministic in `(world_x, world_z)`, so adjacent chunks line up exactly.
/// Result is clamped to `[HEIGHT_MIN, HEIGHT_MAX]`.
pub fn surface_height(world_x: i32, world_z: i32) -> i32 {
    let n = fbm(
        world_x as f64 / TERRAIN_SCALE,
        world_z as f64 / TERRAIN_SCALE,
    );
    let h = SURFACE_Y as f64 + n * TERRAIN_AMPLITUDE;
    (h.round() as i32).clamp(HEIGHT_MIN, HEIGHT_MAX)
}

/// The block state at `world_y` in a column whose surface is at `height`:
/// bedrock floor, stone fill, three dirt layers under the surface, a grass
/// block on top, air above. Bedrock is matched first so the floor is correct
/// regardless of the surface height (it does not rely on `height` staying well
/// above `MIN_Y`).
fn state_at(world_y: i32, height: i32) -> u32 {
    if world_y == MIN_Y {
        states::BEDROCK
    } else if world_y > height {
        states::AIR
    } else if world_y == height {
        states::GRASS_BLOCK
    } else if world_y >= height - 3 {
        states::DIRT
    } else {
        states::STONE
    }
}

// ---------------------------------------------------------------------------
// Per-chunk wire encoding
// ---------------------------------------------------------------------------

/// Compute the 256 column surface heights for chunk `(cx, cz)`, indexed
/// `lz * 16 + lx` to mirror the `(z << 4) | x` part of the cell index.
fn chunk_heights(cx: i32, cz: i32) -> [i32; COLUMNS] {
    let mut heights = [0i32; COLUMNS];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            heights[(lz * 16 + lx) as usize] = surface_height(cx * 16 + lx, cz * 16 + lz);
        }
    }
    heights
}

/// Total world height in blocks (`SECTION_COUNT * 16`), and the exclusive top y.
const WORLD_HEIGHT: i32 = SECTION_COUNT * 16;
const MAX_Y_EXCL: i32 = MIN_Y + WORLD_HEIGHT;

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
    /// broken surface block is represented explicitly).
    edits: HashMap<u32, u32>,
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
    fn state(&self, lx: i32, y: i32, lz: i32) -> u32 {
        if let Some(key) = edit_key(lx, y, lz) {
            if let Some(&s) = self.edits.get(&key) {
                return s;
            }
        }
        state_at(y, self.heights[(lz * 16 + lx) as usize])
    }

    /// Record an edit and invalidate the wire cache, returning the previous state.
    fn set(&mut self, lx: i32, y: i32, lz: i32, state: u32) -> u32 {
        let prev = self.state(lx, y, lz);
        if let Some(key) = edit_key(lx, y, lz) {
            self.edits.insert(key, state);
            self.wire = None;
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
pub fn block_state_at(x: i32, y: i32, z: i32) -> u32 {
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
pub fn set_block(x: i32, y: i32, z: i32, state: u32) -> u32 {
    if !(MIN_Y..MAX_Y_EXCL).contains(&y) {
        return states::AIR;
    }
    let (cx, cz) = (x >> 4, z >> 4);
    let (lx, lz) = (x & 15, z & 15);
    with_chunk(cx, cz, |c| c.set(lx, y, lz, state))
}

/// Encode the 24-section block blob for a chunk from its heights and edits.
fn encode_blob(heights: &[i32; COLUMNS], edits: &HashMap<u32, u32>) -> Vec<u8> {
    let mut out = PacketWriter::new();
    for section in 0..SECTION_COUNT {
        let base_y = MIN_Y + section * 16;
        encode_section(base_y, heights, edits, &mut out);
    }
    out.buf.to_vec()
}

/// Serialize one section: counts, then the block-state and biome containers.
/// `heights` are the 256 per-column surface heights for this chunk, indexed
/// `lz * 16 + lx`; `edits` overrides individual cells.
fn encode_section(
    base_y: i32,
    heights: &[i32; COLUMNS],
    edits: &HashMap<u32, u32>,
    out: &mut PacketWriter,
) {
    // Cell index is vanilla's `(y << 8) | (z << 4) | x`.
    let mut cells = [states::AIR; CELLS];
    let mut non_air: u16 = 0;
    for ly in 0..16i32 {
        let world_y = base_y + ly;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let state = cell_state(heights, edits, lx, world_y, lz);
                if state != states::AIR {
                    non_air += 1;
                }
                let idx = ((ly << 8) | (lz << 4) | lx) as usize;
                cells[idx] = state;
            }
        }
    }

    out.write_i16(non_air as i16); // non-empty block count
    out.write_i16(0); // fluid count
    write_block_palette(&cells, out); // block-state container
    write_single_value(PLAINS_BIOME, out); // biome container (single value)
}

/// The block-state at local `(lx, world_y, lz)`: an edit if present, else the
/// generated terrain state.
fn cell_state(
    heights: &[i32; COLUMNS],
    edits: &HashMap<u32, u32>,
    lx: i32,
    world_y: i32,
    lz: i32,
) -> u32 {
    if let Some(key) = edit_key(lx, world_y, lz) {
        if let Some(&s) = edits.get(&key) {
            return s;
        }
    }
    state_at(world_y, heights[(lz * 16 + lx) as usize])
}

/// Write a block-state `PalettedContainer`. A uniform section collapses to a
/// single-value palette (0 bits, no data array); otherwise we use a 4-bit
/// linear palette — vanilla pads palettes of 1–4 bits up to 4 for block states.
///
/// The width grows with the distinct-state count: 4 bits up to 16 entries, then
/// the smallest width that fits (5..=8 bits, up to 256 entries). Generated
/// terrain uses ≤5 states per section, but block placement can introduce more,
/// so the width is no longer fixed. A section with >256 distinct states (far
/// beyond any realistic manual edit) would need the direct/global palette, which
/// we do not emit; the debug assert guards that ceiling.
fn write_block_palette(cells: &[u32; CELLS], out: &mut PacketWriter) {
    let mut palette: Vec<u32> = Vec::new();
    for &c in cells.iter() {
        if !palette.contains(&c) {
            palette.push(c);
        }
    }

    if palette.len() == 1 {
        write_single_value(palette[0], out);
        return;
    }

    // Indirect (linear) palette: 4 bits minimum, widened to fit the entry count.
    debug_assert!(
        palette.len() <= 256,
        "section palette {} exceeds 8-bit linear capacity",
        palette.len()
    );
    let bits = bits_for_palette(palette.len());
    out.write_u8(bits as u8);
    out.write_varint(palette.len() as i32);
    for &state in &palette {
        out.write_varint(state as i32);
    }
    let indices: Vec<u64> = cells
        .iter()
        .map(|c| palette.iter().position(|p| p == c).unwrap() as u64)
        .collect();
    for long in pack_bits(&indices, bits) {
        out.write_i64(long as i64);
    }
}

/// Bits per entry for an indirect block-state palette of `len` entries: vanilla
/// pads to a 4-bit minimum, then uses the smallest width that indexes `len`.
fn bits_for_palette(len: usize) -> u32 {
    let needed = usize::BITS - (len - 1).leading_zeros();
    needed.max(4)
}

/// A single-value (0-bit) `PalettedContainer`: just the value, no storage.
fn write_single_value(value: u32, out: &mut PacketWriter) {
    out.write_u8(0); // bits per entry
    out.write_varint(value as i32); // the sole palette entry
                                    // No data array follows a 0-bit storage.
}

/// The two client-facing heightmaps (`WORLD_SURFACE` = id 1, `MOTION_BLOCKING`
/// = id 4) for a chunk, each a packed `long[]` of 256 column heights. With no
/// water or non-occluding cover both equal the first free y above the highest
/// non-air block, relative to the world floor (`firstAvailable - minY`).
///
/// Edits are folded in by recomputing each column's top non-air block: an
/// unedited column short-circuits to its generated surface height, while an
/// edited column is scanned from the top so a placed block raises the map and a
/// broken surface lowers it.
fn compute_heightmaps(heights: &[i32; COLUMNS], edits: &HashMap<u32, u32>) -> Vec<(i32, Vec<i64>)> {
    // Bits = ceil(log2(worldHeight + 1)); a 384-tall column -> 9.
    let bits = ((WORLD_HEIGHT + 1) as u32)
        .next_power_of_two()
        .trailing_zeros();
    let mut values = [0u64; COLUMNS];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let col = (lz * 16 + lx) as usize;
            values[col] = (column_first_empty(heights, edits, lx, lz) - MIN_Y) as u64;
        }
    }
    let packed: Vec<i64> = pack_bits(&values, bits)
        .into_iter()
        .map(|l| l as i64)
        .collect();
    vec![(1, packed.clone()), (4, packed)]
}

/// The first empty (air) y above the highest non-air block of column
/// `(lx, lz)`. Unedited columns return `height + 1` directly; columns with edits
/// are scanned downward from the world top. A fully-air column returns `MIN_Y`.
fn column_first_empty(
    heights: &[i32; COLUMNS],
    edits: &HashMap<u32, u32>,
    lx: i32,
    lz: i32,
) -> i32 {
    let height = heights[(lz * 16 + lx) as usize];
    let column_has_edits = edits.keys().any(|&k| {
        let rem = k % COLUMNS as u32;
        rem == (lz as u32) * 16 + lx as u32
    });
    if !column_has_edits {
        return height + 1;
    }
    for y in (MIN_Y..MAX_Y_EXCL).rev() {
        if cell_state(heights, edits, lx, y, lz) != states::AIR {
            return y + 1;
        }
    }
    MIN_Y
}

/// Pack `values` into longs at `bits` each, vanilla `SimpleBitStorage` layout:
/// a value never straddles a long boundary, so each long holds `64 / bits`
/// values low-to-high and any leftover high bits stay zero.
fn pack_bits(values: &[u64], bits: u32) -> Vec<u64> {
    let per_long = (64 / bits) as usize;
    let long_count = values.len().div_ceil(per_long);
    let mut longs = vec![0u64; long_count];
    for (i, &v) in values.iter().enumerate() {
        let long = i / per_long;
        let offset = (i % per_long) as u32 * bits;
        longs[long] |= v << offset;
    }
    longs
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn pack_is_non_spanning_and_low_to_high() {
        // 4-bit values 1,2,3 land in the low nibbles of one long, no spanning.
        let longs = pack_bits(&[1, 2, 3], 4);
        assert_eq!(longs.len(), 1);
        assert_eq!(longs[0], 0x321);
    }

    #[test]
    fn full_section_packs_to_256_longs() {
        // 4096 cells at 4 bits, 16 per long.
        let longs = pack_bits(&[0u64; CELLS], 4);
        assert_eq!(longs.len(), 256);
    }

    #[test]
    fn height_is_deterministic() {
        // Same coordinates -> same height, across calls.
        assert_eq!(surface_height(10, -7), surface_height(10, -7));
        assert_eq!(surface_height(1000, 1000), surface_height(1000, 1000));
    }

    #[test]
    fn height_stays_in_range() {
        // Sweep a wide span and assert every column is in the sane band.
        for x in (-512..512).step_by(7) {
            for z in (-512..512).step_by(13) {
                let h = surface_height(x, z);
                assert!(
                    (HEIGHT_MIN..=HEIGHT_MAX).contains(&h),
                    "height {h} out of range at ({x},{z})"
                );
            }
        }
    }

    #[test]
    fn height_is_continuous_across_a_chunk_boundary() {
        // The last column of chunk 0 and the first of chunk 1 are adjacent
        // world columns; the surface must not jump more than a block or two.
        for z in 0..16 {
            let left = surface_height(15, z); // chunk (0,0) east edge
            let right = surface_height(16, z); // chunk (1,0) west edge
            assert!(
                (left - right).abs() <= 2,
                "discontinuity at z={z}: {left} vs {right}"
            );
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
    fn surface_column_palette_is_within_4_bits() {
        // For every section, confirm the distinct-state count stays within the
        // 16-entry (4-bit) linear-palette ceiling.
        let heights = chunk_heights(0, 0);
        for section in 0..SECTION_COUNT {
            let base_y = MIN_Y + section * 16;
            let mut distinct: Vec<u32> = Vec::new();
            for ly in 0..16i32 {
                for &h in heights.iter() {
                    let s = state_at(base_y + ly, h);
                    if !distinct.contains(&s) {
                        distinct.push(s);
                    }
                }
            }
            assert!(
                distinct.len() <= 16,
                "section {section} has {} states",
                distinct.len()
            );
        }
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
    fn bits_for_palette_widths() {
        assert_eq!(bits_for_palette(2), 4); // padded up to the 4-bit minimum
        assert_eq!(bits_for_palette(16), 4);
        assert_eq!(bits_for_palette(17), 5);
        assert_eq!(bits_for_palette(32), 5);
        assert_eq!(bits_for_palette(33), 6);
        assert_eq!(bits_for_palette(256), 8);
    }
}
