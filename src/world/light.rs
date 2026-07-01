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

/// A chunk's computed light: one entry per light section, ascending from
/// [`MIN_LIGHT_SECTION`]. `Some(bytes)` is a 2048-byte `DataLayer` (nibble per
/// cell); `None` is an all-zero section (vanilla's empty `DataLayer`), which the
/// packet reports via the *empty* mask rather than sending 2048 zero bytes.
pub struct ChunkLight {
    pub sky: Vec<Option<Vec<u8>>>,
    pub block: Vec<Option<Vec<u8>>>,
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
    let dampening = dampening_grid(gen, edits);
    ChunkLight {
        sky: pack_layers(&compute_sky(&dampening)),
        block: pack_layers(&compute_block(gen, edits, &dampening)),
    }
}

/// Precompute the per-cell light dampening (`getLightDampening`) of the whole lit
/// volume in a single pass. Opacity is `max(1, dampening)` and sky seeding needs
/// the transparent (`dampening == 0`) columns, so both derive from this one grid
/// rather than re-fetching each cell's block state.
fn dampening_grid(gen: &GenChunk, edits: &HashMap<u32, BlockState>) -> Vec<u8> {
    let mut grid = vec![0u8; VOLUME];
    for lz in 0..16 {
        for lx in 0..16 {
            for gy in LIGHT_Y_MIN..LIGHT_Y_MAX {
                grid[idx(lx, gy, lz)] = light_dampening(state_for_light(gen, edits, lx, gy, lz));
            }
        }
    }
    grid
}

/// Sky light: seed every column's sky-source cells (at/above `lowestSourceY`) to
/// 15, then flood the shadow with a decreasing BFS.
fn compute_sky(dampening: &[u8]) -> Vec<u8> {
    let mut light = vec![0u8; VOLUME];

    // Direct skylight: scan each column top-down at level 15 while cells are
    // fully transparent; the first cell with any dampening ends the source column
    // (`ChunkSkyLightSources` / `isSourceLevel`). All Vela blocks are fully
    // opaque, so this is exactly "air above the surface is lit".
    for lz in 0..16 {
        for lx in 0..16 {
            for gy in (LIGHT_Y_MIN..LIGHT_Y_MAX).rev() {
                let i = idx(lx, gy, lz);
                if dampening[i] == 0 {
                    light[i] = MAX_LEVEL;
                } else {
                    break;
                }
            }
        }
    }

    // Seed the BFS from source cells on the shadow frontier only — a source that
    // borders a transparent-but-dark cell. Interior sky (open air with lit
    // neighbours) needs no propagation, so terrain with no overhangs enqueues
    // nothing and the flood is a no-op.
    let mut queue: VecDeque<usize> = VecDeque::new();
    for lz in 0..16 {
        for lx in 0..16 {
            for gy in LIGHT_Y_MIN..LIGHT_Y_MAX {
                let i = idx(lx, gy, lz);
                if light[i] == MAX_LEVEL && borders_dark_pervious(&light, dampening, lx, gy, lz) {
                    queue.push_back(i);
                }
            }
        }
    }
    flood(&mut light, dampening, &mut queue);
    light
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

/// Slice a filled [`VOLUME`] light grid into per-section `DataLayer`s. A section
/// that is entirely zero becomes `None` (empty); otherwise its 4096 nibbles are
/// packed into 2048 bytes at `y<<8 | z<<4 | x` (`DataLayer.getIndex`).
fn pack_layers(light: &[u8]) -> Vec<Option<Vec<u8>>> {
    (0..LIGHT_SECTION_COUNT)
        .map(|section| {
            let base_gy = LIGHT_Y_MIN + (section as i32) * 16;
            let mut bytes = vec![0u8; DATA_LAYER_SIZE];
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
                Some(bytes)
            } else {
                None
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
        let sky = compute_sky(&dampening_grid(&gen, &edits));
        let (lx, lz) = (0, 0);
        let surface = 63; // flat plains: grass at y=63
        // Air directly above the surface sees the sky: full 15.
        assert_eq!(sky[idx(lx, surface + 1, lz)], 15);
        assert_eq!(sky[idx(lx, 200, lz)], 15);
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

        let sky = compute_sky(&dampening_grid(&gen, &edits));
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
        assert!(light.block.iter().all(|s| s.is_none()));
    }

    #[test]
    fn top_pad_section_is_full_bright_and_floor_is_empty() {
        let gen = GenChunk::flat(63);
        let edits = HashMap::new();
        let light = compute_light(&gen, &edits);
        // Topmost section (all air, open sky) is a filled 0xFF DataLayer.
        let top = &light.sky[LIGHT_SECTION_COUNT - 1];
        assert!(top.as_ref().is_some_and(|b| b.iter().all(|&x| x == 0xFF)));
        // Bottom pad section (below bedrock) is dark → empty.
        assert!(light.sky[0].is_none());
    }

    #[test]
    fn packing_round_trips_a_known_section() {
        let gen = GenChunk::flat(63);
        let edits = HashMap::new();
        let light = compute_light(&gen, &edits);
        let surface = 63;
        let section = section_of(surface + 1);
        let bytes = light.sky[section].as_ref().expect("lit section present");
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
}
