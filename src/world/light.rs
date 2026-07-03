//! Sky- and block-light computation, producing the `DataLayer` nibble arrays the
//! `ClientboundLightUpdatePacketData` carries alongside a streamed chunk.
//!
//! ## What this mirrors
//!
//! Vanilla lights the world with an *incremental* graph engine
//! (`SkyLightEngine`/`BlockLightEngine` over `DynamicGraphMinFixedPoint`): edits
//! enqueue increase/decrease work and the engine converges over ticks. Vela
//! instead **recomputes a whole chunk's converged light** in one pass when the
//! chunk is (re)built. The wire output — per-section 2048-byte `DataLayer`s — is
//! identical to what the incremental engine reaches at steady state, which is all
//! the client observes.
//!
//! The propagation rules are ported 1:1 from `LightEngine`:
//!
//! * **Opacity** `getOpacity(state) = max(1, lightDampening)` — every step into a
//!   neighbour costs at least 1, so light falls off by one per fully-transparent
//!   block and is fully stopped (cost ≥ 15) by an opaque one
//!   (`LightEngine.MIN_OPACITY`, `MAX_LEVEL = 15`).
//! * **Sky sources** — a column's cells at or above `lowestSourceY` (the first y
//!   with an unobstructed view of the sky, i.e. one above the highest occluder)
//!   are source level 15 and shine straight down without attenuation
//!   (`SkyLightEngine` `ADD_SKY_SOURCE_ENTRY` = increase-skip-UP at 15,
//!   `ChunkSkyLightSources`). From a source, light then spreads in the other five
//!   directions losing `getOpacity` per step.
//! * **Block sources** — a light-emitting block seeds its own `lightEmission` and
//!   spreads it the same way. No block Vela can currently place emits light, so
//!   block light converges to all-zero (every block-light section empty) — the
//!   general emitter path is kept so it lights correctly once emitters exist.
//!
//! ## Known deviation (documented, not silent)
//!
//! Computation is **chunk-local**: a shadowed cell at a chunk border does not
//! receive light bleeding in from the neighbouring chunk (vanilla resolves this
//! with cross-chunk light updates). Vela's generated terrain is solid ground with
//! open sky above and no natural overhangs, so every air cell has direct sky
//! access and the seam is invisible; only edit-made overhangs straddling a chunk
//! boundary would show a one-level difference. Revisit with a world-aware engine
//! when cross-chunk propagation matters.

use std::collections::HashMap;
use std::collections::VecDeque;

use crate::ids::BlockState;

use super::chunk_data::cell_state;
use super::gen::GenChunk;
use super::{states, COLUMNS, MAX_Y_EXCL, MIN_Y, SECTION_COUNT};

/// The lowest light section sits one section *below* the world floor, and the
/// highest one section *above* the build ceiling — sky light exists in the empty
/// section over the world (vanilla `LevelLightEngine` min/max light section).
const MIN_LIGHT_SECTION: i32 = (MIN_Y >> 4) - 1;
/// Number of light sections streamed per chunk: the 24 world sections plus the
/// padding section below and above (`getLightSectionCount()` = sections + 2).
const LIGHT_SECTION_COUNT: usize = SECTION_COUNT as usize + 2;

/// Bottom (inclusive) and top (exclusive) world-y of the lit volume — the light
/// sections span `[MIN_LIGHT_SECTION*16, (MIN_LIGHT_SECTION+COUNT)*16)`.
const LIGHT_Y_MIN: i32 = MIN_LIGHT_SECTION * 16;
const LIGHT_Y_MAX: i32 = LIGHT_Y_MIN + (LIGHT_SECTION_COUNT as i32) * 16;
/// Vertical cell count of the lit volume (`LIGHT_SECTION_COUNT * 16`).
const LIGHT_HEIGHT: usize = LIGHT_SECTION_COUNT * 16;
/// Cells in one light volume: 16×16 columns over the full lit height.
const VOLUME: usize = LIGHT_HEIGHT * COLUMNS;
/// Bytes in one section's `DataLayer` (4096 cells × 4 bits).
const DATA_LAYER_SIZE: usize = 2048;
/// Maximum light level (`LightEngine.MAX_LEVEL`).
const MAX_LEVEL: u8 = 15;

/// One light section's `DataLayer`. Beyond vanilla's present/empty split this
/// distinguishes the *uniformly full-bright* open-sky section: every chunk's
/// sections above its terrain are the identical all-`0xFF` layer, so they are
/// served from the shared [`FULL_BRIGHT_LAYER`] instead of each chunk owning a
/// 2048-byte copy (~15 such sections per chunk — ~30 KiB/chunk resident at
/// large view distances). Wire output is unchanged: [`LightLayer::bytes`]
/// yields exactly the bytes the old `Option<Vec<u8>>` held.
pub enum LightLayer {
    /// All-zero (vanilla's empty `DataLayer`) — the packet reports it via the
    /// *empty* mask rather than sending 2048 zero bytes.
    Empty,
    /// Uniform open-sky full-bright — identical bytes for every chunk, read
    /// from the shared static.
    Bright,
    /// A computed 2048-byte `DataLayer` (nibble per cell).
    Data(Box<[u8; DATA_LAYER_SIZE]>),
}

impl LightLayer {
    /// The section's wire bytes: `None` for an empty section, otherwise the
    /// 2048-byte `DataLayer` (shared static for [`LightLayer::Bright`]).
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            LightLayer::Empty => None,
            LightLayer::Bright => Some(&FULL_BRIGHT_LAYER),
            LightLayer::Data(bytes) => Some(&**bytes),
        }
    }
}

/// A chunk's computed light: one entry per light section, ascending from
/// [`MIN_LIGHT_SECTION`].
pub struct ChunkLight {
    pub sky: Vec<LightLayer>,
    pub block: Vec<LightLayer>,
}

impl ChunkLight {
    /// `LevelReader.getRawBrightness(pos, 0)` — the brightness seen at a world
    /// position with **no** sky-darkening subtracted (`amount == 0`):
    /// `max(skyLight, blockLight)`. `(lx, lz)` are chunk-relative `0..16`; a `y`
    /// outside the lit volume (or a section stored empty) reads as dark `0`.
    /// Used by the natural spawner's `Animal.isBrightEnoughToSpawn` gate
    /// (`getRawBrightness(pos, 0) > 8`).
    pub fn raw_brightness(&self, lx: i32, y: i32, lz: i32) -> u8 {
        section_nibble(&self.sky, lx, y, lz).max(section_nibble(&self.block, lx, y, lz))
    }

    /// Heap bytes across both layers' stored sections — the per-chunk light
    /// term the memory profiler tracks. `Bright` sections share one static and
    /// so cost nothing per chunk.
    pub fn heap_bytes(&self) -> usize {
        let layers = |v: &[LightLayer]| {
            v.iter()
                .map(|l| match l {
                    LightLayer::Data(bytes) => std::mem::size_of_val(&**bytes),
                    LightLayer::Empty | LightLayer::Bright => 0,
                })
                .sum::<usize>()
        };
        layers(&self.sky) + layers(&self.block)
    }
}

/// Read a single nibble out of a per-section `DataLayer` list at chunk-relative
/// `(lx, lz)` and world-`y`, mirroring the `y<<8 | z<<4 | x` packing in
/// [`pack_layers`]. An out-of-range `y` or an empty (`None`) section reads `0`.
fn section_nibble(layers: &[LightLayer], lx: i32, y: i32, lz: i32) -> u8 {
    if !(LIGHT_Y_MIN..LIGHT_Y_MAX).contains(&y) {
        return 0;
    }
    let section = ((y - LIGHT_Y_MIN) / 16) as usize;
    let Some(bytes) = layers.get(section).and_then(LightLayer::bytes) else {
        return 0;
    };
    let ly = y - (LIGHT_Y_MIN + section as i32 * 16);
    let cell = ((ly << 8) | (lz << 4) | lx) as usize;
    (bytes[cell >> 1] >> (4 * (cell & 1))) & 0xF
}

/// Light this block state dampens, as `getOpacity` sees it *before* the `max(1,
/// …)` floor: 0 for fully transparent cells (air, plants, thin snow), 1 for
/// translucent blocks (water, ice, leaves) that let light through attenuating by
/// one, 15 for opaque solids. Delegates to the generator's block classification
/// so surface/decoration blocks light correctly.
fn light_dampening(state: BlockState) -> u8 {
    super::gen::light_dampening(state)
}

/// The light level a block state emits (`getLightEmission`). No block Vela can
/// currently place emits light, so this is always 0; wired through so the block
/// engine lights correctly the moment glowstone/torches/lava exist.
fn light_emission(_state: BlockState) -> u8 {
    0
}

/// `getOpacity = max(1, lightDampening)` — the per-step cost of entering a cell,
/// at least [`LightEngine.MIN_OPACITY`](1). Takes the cell's precomputed dampening
/// (from [`dampening_grid`]) so the flood never re-fetches a block state.
#[inline]
fn opacity(dampening: u8) -> u8 {
    dampening.max(1)
}

/// Local index into a [`VOLUME`]-sized light array for chunk-relative `(lx, lz)`
/// and world-y `gy` (already known to be in `[LIGHT_Y_MIN, LIGHT_Y_MAX)`).
#[inline]
fn idx(lx: i32, gy: i32, lz: i32) -> usize {
    let vy = (gy - LIGHT_Y_MIN) as usize;
    (vy * 16 + lz as usize) * 16 + lx as usize
}

/// The block state at chunk-relative `(lx, gy, lz)` for lighting purposes:
/// generated terrain or an edit inside the world, air above the build ceiling,
/// and solid (bedrock-like) below the floor — matching vanilla's `getState`,
/// which returns bedrock for a missing/out-of-range position.
fn state_for_light(
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
    lx: i32,
    gy: i32,
    lz: i32,
) -> BlockState {
    if gy >= MAX_Y_EXCL {
        states::AIR
    } else if gy < MIN_Y {
        states::BEDROCK
    } else {
        cell_state(gen, edits, lx, gy, lz)
    }
}

/// The six propagation directions as `(dx, dy, dz)` (`PROPAGATION_DIRECTIONS`).
const NEIGHBORS: [(i32, i32, i32); 6] = [
    (0, 1, 0),
    (0, -1, 0),
    (1, 0, 0),
    (-1, 0, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Compute both light layers for a chunk from its column heights and edits.
pub(super) fn compute_light(
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
) -> ChunkLight {
    // One state lookup per cell: the whole flood reads dampening from here
    // (`opacity = max(1, dampening)`) and sky seeding keys off `dampening == 0`.
    // The same pass also yields each column's highest occluder (`col_ceiling`),
    // which lets sky light skip the uniformly-lit open air above the terrain.
    let (dampening, col_ceiling) = dampening_grid(gen, edits);
    let (sky, bright_from_section) = compute_sky(&dampening, &col_ceiling);
    ChunkLight {
        sky: pack_sky(&sky, bright_from_section),
        block: pack_layers(&compute_block(gen, edits, &dampening)),
    }
}

/// Precompute the per-cell light dampening (`getLightDampening`) of the whole lit
/// volume in a single pass. Opacity is `max(1, dampening)` and sky seeding needs
/// the transparent (`dampening == 0`) columns, so both derive from this one grid
/// rather than re-fetching each cell's block state.
///
/// The `gy` loop is the **outermost** so array writes stride by 1 — `idx` packs
/// as `(vy*16 + lz)*16 + lx`, so a fixed `gy` with `lz`/`lx` inner walks the grid
/// sequentially (cache-friendly) instead of jumping 256 cells per step.
///
/// Alongside the grid it returns each column's `col_ceiling`: the highest world-y
/// carrying any dampening (opaque/translucent) block — i.e. its highest occluder.
/// Every cell strictly above `col_ceiling` is fully transparent and so a direct
/// sky source; this is the edit-aware equivalent of `GenChunk::world_surface_top`
/// (`ws_top`) and stays correct when edits add/remove blocks the heightmap misses.
/// `LIGHT_Y_MIN - 1` sentinels a fully transparent column (all sky source).
fn dampening_grid(
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
) -> (Vec<u8>, [i32; COLUMNS]) {
    let mut grid = vec![0u8; VOLUME];
    let mut col_ceiling = [LIGHT_Y_MIN - 1; COLUMNS];
    for gy in LIGHT_Y_MIN..LIGHT_Y_MAX {
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let d = light_dampening(state_for_light(gen, edits, lx, gy, lz));
                grid[idx(lx, gy, lz)] = d;
                if d > 0 {
                    // `gy` ascends, so the last write per column is its highest occluder.
                    col_ceiling[(lz * 16 + lx) as usize] = gy;
                }
            }
        }
    }
    (grid, col_ceiling)
}

/// Sky light: seed every column's sky-source cells (at/above `lowestSourceY`) to
/// 15, then flood the shadow with a decreasing BFS.
///
/// Returns the filled light grid plus `bright_from_section`: the first light
/// section index that is entirely above the tallest occluder. Those top sections
/// are uniform sky-source 15 (no frontier, nothing to flood), so the grid is left
/// unfilled there and [`pack_sky`] serves them from a shared full-bright layer.
fn compute_sky(dampening: &[u8], col_ceiling: &[i32]) -> (Vec<u8>, usize) {
    let mut light = vec![0u8; VOLUME];

    // Tallest / shortest column occluder across the chunk. Everything strictly
    // above `global_max_top` is open sky in *every* column (uniform source 15);
    // no source exists at or below `global_min_top`.
    let global_max_top = col_ceiling.iter().copied().max().unwrap_or(LIGHT_Y_MIN - 1);
    let global_min_top = col_ceiling.iter().copied().min().unwrap_or(LIGHT_Y_MIN - 1);

    // Sections whose base lies above `global_max_top` are uniformly full-bright.
    let bright_from_section = ((global_max_top - LIGHT_Y_MIN) / 16 + 1) as usize;
    let bright_from_gy = LIGHT_Y_MIN + bright_from_section as i32 * 16;

    // Direct skylight (`ChunkSkyLightSources` / `isSourceLevel`): every cell
    // strictly above a column's occluder is a level-15 source. We only need the
    // grid filled up to the base of the shared region — the one sentinel row at
    // `bright_from_gy` keeps the flood/frontier from mis-reading the skipped
    // uniform cells above it as dark. (Clamped to stay in-bounds; when there is no
    // shared region this fills the whole open column exactly as before.)
    let fill_top = bright_from_gy.min(LIGHT_Y_MAX - 1);
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let ceil = col_ceiling[(lz * 16 + lx) as usize];
            let lo = (ceil + 1).max(LIGHT_Y_MIN);
            for gy in lo..=fill_top {
                light[idx(lx, gy, lz)] = MAX_LEVEL;
            }
        }
    }

    // Seed the BFS from source cells on the shadow frontier only — a source that
    // borders a transparent-but-dark cell. Such a source must (a) be a source, so
    // it sits strictly above its own column's occluder, hence `gy > global_min_top`;
    // and (b) border a darker light-passing cell. Its horizontal/upward neighbours
    // are all sources once `gy > global_max_top`, but its **downward** neighbour is
    // the occluder at `gy - 1`, and an occluder can be *translucent*: water counts
    // toward `col_ceiling` (`getLightDampening() == 1 != 0`, exactly vanilla
    // `ChunkSkyLightSources.isEdgeOccluded`) yet still passes light. So the source
    // row directly above the tallest occluder (`global_max_top + 1`) is a real
    // frontier — it must seed the downward flood into a submerged column — and the
    // band has to reach it. Frontier candidates therefore lie in
    // `(global_min_top, global_max_top + 1]`; opaque occluders make that top row a
    // no-op (their `gy-1` neighbour has opacity 15, so `borders_dark_pervious` is
    // false), so flat solid terrain still enqueues nothing.
    let band_lo = (global_min_top + 1).max(LIGHT_Y_MIN);
    // `global_max_top + 1` is always at or below `fill_top` (the filled region
    // ends at `bright_from_gy >= global_max_top + 1`), so the clamp only guards the
    // degenerate near-ceiling case; the scanned sources are genuinely filled to 15.
    let band_hi = (global_max_top + 1).min(fill_top);
    let mut queue: VecDeque<usize> = VecDeque::new();
    for gy in band_lo..=band_hi {
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let i = idx(lx, gy, lz);
                if light[i] == MAX_LEVEL && borders_dark_pervious(&light, dampening, lx, gy, lz) {
                    queue.push_back(i);
                }
            }
        }
    }
    flood(&mut light, dampening, &mut queue);
    (light, bright_from_section)
}

/// Block light: seed each emitting block with its emission level, then flood.
/// With no emitters modelled this produces an all-zero grid (all sections empty).
fn compute_block(
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
    dampening: &[u8],
) -> Vec<u8> {
    let mut light = vec![0u8; VOLUME];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for lz in 0..16 {
        for lx in 0..16 {
            for gy in LIGHT_Y_MIN..LIGHT_Y_MAX {
                let emission = light_emission(state_for_light(gen, edits, lx, gy, lz));
                if emission > 0 {
                    let i = idx(lx, gy, lz);
                    light[i] = emission;
                    queue.push_back(i);
                }
            }
        }
    }
    flood(&mut light, dampening, &mut queue);
    light
}

/// True if this max-level source cell has at least one in-bounds neighbour the
/// flood could still raise — i.e. a light-passing (`opacity < 15`) neighbour whose
/// current level is below what a step from a 15-source would give it
/// (`15 - opacity`). Keeps the BFS seed to the shadow boundary. Written in terms
/// of opacity (not a hard-coded `== 1`) so it stays correct once translucent
/// blocks with intermediate dampening exist.
fn borders_dark_pervious(light: &[u8], dampening: &[u8], lx: i32, gy: i32, lz: i32) -> bool {
    for (dx, dy, dz) in NEIGHBORS {
        let (nx, ny, nz) = (lx + dx, gy + dy, lz + dz);
        if !(0..16).contains(&nx) || !(0..16).contains(&nz) || !(LIGHT_Y_MIN..LIGHT_Y_MAX).contains(&ny) {
            continue;
        }
        let ni = idx(nx, ny, nz);
        let op = opacity(dampening[ni]);
        if op < MAX_LEVEL && light[ni] < MAX_LEVEL - op {
            return true;
        }
    }
    false
}

/// Decreasing breadth-first flood: pop a cell at level `l`, and for each in-bounds
/// neighbour compute `l - getOpacity(neighbour)`; if that beats the neighbour's
/// current level, raise it and re-enqueue. Mirrors the converged state of
/// `LightEngine`'s increase queue (`getLightDampeningInto` with simple opacity).
fn flood(light: &mut [u8], dampening: &[u8], queue: &mut VecDeque<usize>) {
    while let Some(i) = queue.pop_front() {
        let level = light[i];
        if level <= 1 {
            continue; // nothing a neighbour could receive (min step cost is 1)
        }
        // Recover (lx, gy, lz) from the flat index.
        let lx = (i % 16) as i32;
        let lz = ((i / 16) % 16) as i32;
        let gy = (i / COLUMNS) as i32 + LIGHT_Y_MIN;
        for (dx, dy, dz) in NEIGHBORS {
            let (nx, ny, nz) = (lx + dx, gy + dy, lz + dz);
            if !(0..16).contains(&nx)
                || !(0..16).contains(&nz)
                || !(LIGHT_Y_MIN..LIGHT_Y_MAX).contains(&ny)
            {
                continue;
            }
            let ni = idx(nx, ny, nz);
            let next = level.saturating_sub(opacity(dampening[ni]));
            if next > light[ni] {
                light[ni] = next;
                queue.push_back(ni);
            }
        }
    }
}

/// A section's worth of nibbles, all `15` — the packed form of uniformly open
/// sky. One static shared by every chunk's [`LightLayer::Bright`] sections, so
/// full-bright sections cost no per-chunk memory and no packing work.
static FULL_BRIGHT_LAYER: [u8; DATA_LAYER_SIZE] = [0xFF; DATA_LAYER_SIZE];

/// Pack one section of a filled [`VOLUME`] light grid into a `DataLayer`.
/// Returns [`LightLayer::Empty`] for an all-zero section; otherwise its 4096
/// nibbles are packed into 2048 bytes at `y<<8 | z<<4 | x` (`DataLayer.getIndex`).
fn pack_section(light: &[u8], section: usize) -> LightLayer {
    let base_gy = LIGHT_Y_MIN + (section as i32) * 16;
    let mut bytes = Box::new([0u8; DATA_LAYER_SIZE]);
    let mut any = false;
    for ly in 0..16i32 {
        let gy = base_gy + ly;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let value = light[idx(lx, gy, lz)];
                if value == 0 {
                    continue;
                }
                any = true;
                let cell = ((ly << 8) | (lz << 4) | lx) as usize;
                // Even cell → low nibble, odd cell → high nibble.
                bytes[cell >> 1] |= value << (4 * (cell & 1));
            }
        }
    }
    if any {
        LightLayer::Data(bytes)
    } else {
        LightLayer::Empty
    }
}

/// Slice a filled [`VOLUME`] light grid into per-section `DataLayer`s.
fn pack_layers(light: &[u8]) -> Vec<LightLayer> {
    (0..LIGHT_SECTION_COUNT)
        .map(|section| pack_section(light, section))
        .collect()
}

/// Like [`pack_layers`], but sections at or above `bright_from_section` are known
/// to be uniformly open sky (source 15) — [`compute_sky`] never fills the grid
/// there. Those become [`LightLayer::Bright`] (the shared [`FULL_BRIGHT_LAYER`],
/// byte-identical to nibble-packing all-15) instead of an owned copy per chunk.
fn pack_sky(light: &[u8], bright_from_section: usize) -> Vec<LightLayer> {
    (0..LIGHT_SECTION_COUNT)
        .map(|section| {
            if section >= bright_from_section {
                LightLayer::Bright
            } else {
                pack_section(light, section)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::gen::GenChunk;

    /// Read a nibble back out of a packed `DataLayer` at cell `(lx, ly, lz)`.
    fn nibble(bytes: &[u8], lx: i32, ly: i32, lz: i32) -> u8 {
        let cell = ((ly << 8) | (lz << 4) | lx) as usize;
        (bytes[cell >> 1] >> (4 * (cell & 1))) & 0xF
    }

    /// The light-section index (0-based, from MIN_LIGHT_SECTION) holding world-y.
    fn section_of(gy: i32) -> usize {
        ((gy - LIGHT_Y_MIN) / 16) as usize
    }

    #[test]
    fn section_geometry_matches_vanilla_overworld() {
        // 24 world sections + 1 below + 1 above; the pad section starts at -80.
        assert_eq!(LIGHT_SECTION_COUNT, 26);
        assert_eq!(MIN_LIGHT_SECTION, -5);
        assert_eq!(LIGHT_Y_MIN, -80);
        assert_eq!(LIGHT_Y_MAX, 336);
    }

    #[test]
    fn open_column_is_fully_sky_lit_above_surface_and_dark_below() {
        let gen = GenChunk::flat(63);
        let edits = HashMap::new();
        let (dampening, col_ceiling) = dampening_grid(&gen, &edits);
        let (sky, bright_from_section) = compute_sky(&dampening, &col_ceiling);
        let (lx, lz) = (0, 0);
        let surface = 63; // flat plains: grass at y=63
        // Air directly above the surface sees the sky: full 15 (computed band).
        assert_eq!(sky[idx(lx, surface + 1, lz)], 15);
        // High open air lives in the skipped uniform region: the grid is left
        // unfilled there, but the *packed* output (what the client sees) is 15.
        let light = compute_light(&gen, &edits);
        let section = section_of(200);
        assert!(section >= bright_from_section);
        let bytes = light.sky[section].bytes().expect("open-sky section present");
        let ly = 200 - (LIGHT_Y_MIN + section as i32 * 16);
        assert_eq!(nibble(bytes, lx, ly, lz), 15);
        // The surface block and everything below it are unlit.
        assert_eq!(sky[idx(lx, surface, lz)], 0);
        assert_eq!(sky[idx(lx, surface - 3, lz)], 0);
    }

    #[test]
    fn overhang_casts_an_attenuating_shadow() {
        // Float a single opaque block high above an open column: the cell right
        // under it loses direct sky and is lit only by horizontal bleed from its
        // four open (15) neighbours, so it reads one step down at 14.
        let gen = GenChunk::flat(63);
        let mut edits = HashMap::new();
        let roof_y = 150;
        edits.insert(edit_key_for(8, roof_y, 8), states::STONE);

        let (dampening, col_ceiling) = dampening_grid(&gen, &edits);
        let sky = compute_sky(&dampening, &col_ceiling).0;
        // Directly beneath the block: shadowed, one step in from open sky → 14.
        assert_eq!(sky[idx(8, roof_y - 1, 8)], 14);
        // Its neighbours (and open air elsewhere) still see full sky.
        assert_eq!(sky[idx(9, roof_y - 1, 8)], 15);
        assert_eq!(sky[idx(12, roof_y - 1, 12)], 15);
    }

    #[test]
    fn no_emitters_means_block_light_is_all_empty() {
        let gen = GenChunk::flat(63);
        let edits = HashMap::new();
        let light = compute_light(&gen, &edits);
        assert!(light.block.iter().all(|s| s.bytes().is_none()));
    }

    #[test]
    fn top_pad_section_is_full_bright_and_floor_is_empty() {
        let gen = GenChunk::flat(63);
        let edits = HashMap::new();
        let light = compute_light(&gen, &edits);
        // Topmost section (all air, open sky) is a filled 0xFF DataLayer.
        let top = &light.sky[LIGHT_SECTION_COUNT - 1];
        assert!(top.bytes().is_some_and(|b| b.iter().all(|&x| x == 0xFF)));
        // Bottom pad section (below bedrock) is dark → empty.
        assert!(light.sky[0].bytes().is_none());
    }

    #[test]
    fn packing_round_trips_a_known_section() {
        let gen = GenChunk::flat(63);
        let edits = HashMap::new();
        let light = compute_light(&gen, &edits);
        let surface = 63;
        let section = section_of(surface + 1);
        let bytes = light.sky[section].bytes().expect("lit section present");
        // The air cell above the surface in column (0,0) reads back as 15.
        let ly = (surface + 1 - (LIGHT_Y_MIN + section as i32 * 16)) as i32;
        assert_eq!(nibble(bytes, 0, ly, 0), 15);
    }

    /// Mirror of `chunk_data::edit_key` for the tests that seed overhang edits.
    fn edit_key_for(lx: i32, y: i32, lz: i32) -> u32 {
        ((y - MIN_Y) as u32) * COLUMNS as u32 + (lz as u32) * 16 + lx as u32
    }

    #[test]
    fn frontier_seed_covers_intermediate_opacity_neighbours() {
        // Guards the future translucent-block path: a 15-source bordering a cell
        // whose dampening is neither 0 nor 15 (e.g. a slab/water-like `getOpacity`
        // of 5) must still be seeded — the old `opacity == 1` test dropped it, so
        // that shadow would never receive `15 - 5 = 10`.
        let mut light = vec![0u8; VOLUME];
        let mut dampening = vec![0u8; VOLUME];
        let (sx, sy, sz) = (5, 100, 5);
        light[idx(sx, sy, sz)] = MAX_LEVEL;

        // A neighbour with intermediate opacity (5) sitting dark → real work.
        dampening[idx(sx + 1, sy, sz)] = 5;
        assert!(borders_dark_pervious(&light, &dampening, sx, sy, sz));

        // Walled in by fully-opaque neighbours (opacity 15) on all sides, the
        // source can raise nothing → not seeded.
        let opaque = vec![MAX_LEVEL; VOLUME];
        assert!(!borders_dark_pervious(&light, &opaque, sx, sy, sz));

        // A one-step flood from the source raises the intermediate cell to 10.
        let mut queue: VecDeque<usize> = VecDeque::new();
        queue.push_back(idx(sx, sy, sz));
        flood(&mut light, &dampening, &mut queue);
        assert_eq!(light[idx(sx + 1, sy, sz)], MAX_LEVEL - 5);
    }

    /// Fill every column of a `flat(0)` chunk with water from y=1 up to and
    /// including `water_top`, leaving open air above — a uniformly deep,
    /// **fully submerged** chunk. Returns the edit map.
    fn submerged_edits(water_top: i32) -> HashMap<u32, BlockState> {
        let water = crate::world::gen::water_state();
        let mut edits = HashMap::new();
        for lz in 0..16 {
            for lx in 0..16 {
                for y in 1..=water_top {
                    edits.insert(edit_key_for(lx, y, lz), water);
                }
            }
        }
        edits
    }

    #[test]
    fn fully_submerged_chunk_attenuates_skylight_per_water_block() {
        // A chunk whose entire 16×16 top is water at the same y (deep ocean
        // interior). Vanilla `ChunkSkyLightSources.isEdgeOccluded` treats water
        // as an occluder (`getLightDampening() == 1 != 0`), so the lowest sky
        // source is the *air cell just above the topmost water* (15); the water
        // below is lit by downward propagation losing `getOpacity(water) = 1`
        // per block — 14, 13, … down to 1, then dark. Regression: the band-scan
        // seed skipped uniform-depth translucent columns, leaving the water at 0.
        let gen = GenChunk::flat(0);
        let edits = submerged_edits(62); // topmost water at y=62, air at y=63+
        let light = compute_light(&gen, &edits);

        // Air just above the water surface is a full sky source.
        assert_eq!(light.raw_brightness(8, 63, 8), 15);
        // Each water block down the column loses exactly one level.
        assert_eq!(light.raw_brightness(8, 62, 8), 14);
        assert_eq!(light.raw_brightness(8, 61, 8), 13);
        assert_eq!(light.raw_brightness(8, 55, 8), 7);
        assert_eq!(light.raw_brightness(8, 49, 8), 1);
        // Fourteen blocks down the light has reached 0 and stops.
        assert_eq!(light.raw_brightness(8, 48, 8), 0);
    }

    #[test]
    fn partially_submerged_chunk_still_lights_water() {
        // Control: the same water, but one column is dry land poking above the
        // surface, so occluder heights vary (chunk only *partially* submerged).
        // This case was always lit correctly; it must stay that way.
        let gen = GenChunk::flat(0);
        let mut edits = submerged_edits(62);
        for y in 1..=90 {
            edits.insert(edit_key_for(0, y, 0), states::STONE);
        }
        let light = compute_light(&gen, &edits);

        // A water column away from the pillar attenuates just like the fully
        // submerged case.
        assert_eq!(light.raw_brightness(8, 62, 8), 14);
        assert_eq!(light.raw_brightness(8, 61, 8), 13);
        assert_eq!(light.raw_brightness(8, 49, 8), 1);
        assert_eq!(light.raw_brightness(8, 48, 8), 0);
    }
}
