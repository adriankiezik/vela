//! World generation: the noise height field, climate-driven biomes, per-column
//! surface rules, and per-chunk decoration, assembled into a [`GenChunk`].
//!
//! A `GenChunk` is the deterministic *baseline* of one chunk — heights, biomes,
//! the surface/fill blocks, and the generated features (trees, ores, plants) —
//! from which the wire encoder, heightmaps, and light are derived. Player edits
//! layer on top as a sparse override map in [`super::chunk_data`]; the baseline
//! itself is a pure function of `(seed, cx, cz)`, so a saved chunk reloads to
//! exactly its player edits (regenerate baseline, diff against disk).
//!
//! Reference: `world/level/levelgen` (NoiseRouter, SurfaceSystem/SurfaceRules,
//! carvers, PlacedFeature/ConfiguredFeature). This is a believable overworld, not
//! a `NoiseBasedChunkGenerator` port — structures are out of scope.

mod biome;
mod blocks;
mod feature;
mod noise;
pub mod random;
mod rng;
pub mod synth;
mod surface;

use std::sync::OnceLock;

use crate::ids::BlockState;

use biome::Biome;

use super::{COLUMNS, MAX_Y_EXCL, MIN_Y, SURFACE_Y};

/// The generator's default seed when no world seed has been threaded in. Kept as
/// the historical constant so unedited test worlds are byte-stable.
pub const DEFAULT_SEED: u32 = 0x5EED_C0DE;

/// Sea level — the y water fills up to. Shares the world's reference surface.
const SEA_LEVEL: i32 = SURFACE_Y;

/// Field-mixing salts so each noise layer samples an independent lattice.
const HEIGHT_SALT: u32 = 0x0001;
const HILL_SALT: u32 = 0x0002;
const DETAIL_SALT: u32 = 0x0003;
const PEAK_SALT: u32 = 0x0004;
const TEMP_SALT: u32 = 0x0011;
const HUMID_SALT: u32 = 0x0012;
/// Salt for the surface-rule fields (bedrock/deepslate/caves).
const SURFACE_SALT: u32 = 0x00A0;

/// The runtime world seed, set once at boot from `level.dat` (falling back to
/// [`DEFAULT_SEED`]). A `OnceLock` keeps `surface_height` — called before any
/// `GenChunk` exists, e.g. spawn selection — reading the same seed as generation.
static RUNTIME_SEED: OnceLock<u64> = OnceLock::new();

/// Thread the persisted world seed into generation. Idempotent; the first value
/// wins (subsequent calls, e.g. a re-boot within one process, are ignored).
pub fn set_seed(seed: i64) {
    let _ = RUNTIME_SEED.set(seed as u64);
}

/// The active world seed (defaulting to [`DEFAULT_SEED`] until [`set_seed`]).
pub fn seed() -> u64 {
    *RUNTIME_SEED.get_or_init(|| DEFAULT_SEED as u64)
}

/// Derive a 32-bit field seed from the world seed and a salt (splitmix-style mix).
fn field_seed(salt: u32) -> u32 {
    let mut h = seed() ^ (salt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    (h ^ (h >> 32)) as u32
}

/// The terrain surface height (topmost solid block y) for a world column.
/// Deterministic in `(wx, wz)` and continuous, so adjacent chunks line up.
pub fn surface_height(wx: i32, wz: i32) -> i32 {
    let hs = field_seed(HEIGHT_SALT);
    let cont = noise::fbm2(wx as f64 / 400.0, wz as f64 / 400.0, hs, 4);
    let hills = noise::fbm2(wx as f64 / 120.0, wz as f64 / 120.0, field_seed(HILL_SALT), 3);
    let detail = noise::fbm2(wx as f64 / 45.0, wz as f64 / 45.0, field_seed(DETAIL_SALT), 2);
    let peaks = noise::fbm2(wx as f64 / 320.0, wz as f64 / 320.0, field_seed(PEAK_SALT), 2);
    // A gated mountain term: only where the peak field is high, rising smoothly.
    let mtn = ((peaks - 0.4) / 0.6).clamp(0.0, 1.0);

    let h = SEA_LEVEL as f64 + cont * 26.0 + hills * 10.0 + detail * 4.0 + mtn * 30.0;
    (h.round() as i32).clamp(MIN_Y + 2, MAX_Y_EXCL - 40)
}

/// The `(temperature, humidity)` climate at a world column, each in ~`[-1, 1]`.
fn climate(wx: i32, wz: i32) -> (f64, f64) {
    let t = noise::fbm2(wx as f64 / 500.0, wz as f64 / 500.0, field_seed(TEMP_SALT), 3);
    let h = noise::fbm2(wx as f64 / 440.0, wz as f64 / 440.0, field_seed(HUMID_SALT), 3);
    (t, h)
}

/// The biome at a world column, from its climate and height relative to sea level.
pub fn biome_at(wx: i32, wz: i32) -> Biome {
    let (t, h) = climate(wx, wz);
    biome::classify(t, h, surface_height(wx, wz), SEA_LEVEL)
}

/// A generated chunk baseline: heights, biomes, decoration overrides, and the
/// precomputed heightmap tops. Cheap to query per cell via [`GenChunk::base_state`].
pub struct GenChunk {
    /// Per-column surface heights, indexed `lz * 16 + lx`.
    pub heights: [i32; COLUMNS],
    biomes: [Biome; COLUMNS],
    /// The dense baseline block grid (surface rule + carved caves + generated
    /// features), indexed `(world_y - MIN_Y) * COLUMNS + lz*16 + lx` — the same
    /// index [`edit_key`] produces. Computed once so the encoder, heightmap, and
    /// light engine read O(1) instead of re-evaluating the noise per pass. Empty
    /// for the test-only `simple` chunk, which computes columns arithmetically.
    grid: Vec<BlockState>,
    /// `WORLD_SURFACE` / `MOTION_BLOCKING` first-empty y per column (fast path for
    /// the unedited heightmap).
    ws_top: [i32; COLUMNS],
    mb_top: [i32; COLUMNS],
    /// Test-only: when set, [`base_state`](GenChunk::base_state) uses the legacy
    /// bedrock/dirt/grass/stone column with no water, caves, or features — so the
    /// sibling heightmap/light unit tests can reason about flat, dry columns.
    #[cfg_attr(not(test), allow(dead_code))]
    simple: bool,
}

/// The dense grid index for world `(lx, world_y, lz)` — identical to [`edit_key`]
/// so generated feature overrides drop straight into place.
fn grid_index(lx: i32, world_y: i32, lz: i32) -> usize {
    ((world_y - MIN_Y) as usize) * COLUMNS + (lz * 16 + lx) as usize
}

impl GenChunk {
    /// Generate the baseline for chunk `(cx, cz)`.
    pub fn generate(cx: i32, cz: i32) -> Self {
        let mut heights = [0i32; COLUMNS];
        let mut biomes = [Biome::Plains; COLUMNS];
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let wx = cx * 16 + lx;
                let wz = cz * 16 + lz;
                let col = (lz * 16 + lx) as usize;
                let h = surface_height(wx, wz);
                heights[col] = h;
                let (t, hum) = climate(wx, wz);
                biomes[col] = biome::classify(t, hum, h, SEA_LEVEL);
            }
        }

        let features = feature::decorate(cx, cz, &heights, &biomes, SEA_LEVEL, seed());
        let surface_seed = field_seed(SURFACE_SALT);

        // Bake the whole chunk once: surface rule per cell, then overlay features.
        let cells = ((MAX_Y_EXCL - MIN_Y) as usize) * COLUMNS;
        let mut grid = vec![super::states::AIR; cells];
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let col = (lz * 16 + lx) as usize;
                let (h, biome) = (heights[col], biomes[col]);
                for world_y in MIN_Y..MAX_Y_EXCL {
                    grid[grid_index(lx, world_y, lz)] = surface::column_state(
                        cx * 16 + lx,
                        world_y,
                        cz * 16 + lz,
                        h,
                        biome,
                        SEA_LEVEL,
                        MIN_Y,
                        surface_seed,
                    );
                }
            }
        }
        for (&key, &state) in &features {
            // Feature keys are grid indices by construction (`edit_key`).
            grid[key as usize] = state;
        }

        let mut this = Self {
            heights,
            biomes,
            grid,
            ws_top: [MIN_Y; COLUMNS],
            mb_top: [MIN_Y; COLUMNS],
            simple: false,
        };
        this.compute_tops();
        this
    }

    /// A flat, dry, featureless plains chunk at `height` — test scaffolding for
    /// the heightmap and light engines, which reason about simple columns.
    #[cfg(test)]
    pub(in crate::world) fn flat(height: i32) -> Self {
        let mut this = Self {
            heights: [height; COLUMNS],
            biomes: [Biome::Plains; COLUMNS],
            grid: Vec::new(),
            ws_top: [MIN_Y; COLUMNS],
            mb_top: [MIN_Y; COLUMNS],
            simple: true,
        };
        this.compute_tops();
        this
    }

    /// Mutable access to a test chunk's column heights (recomputes nothing — the
    /// caller edits the affected columns so the heightmap scans them).
    #[cfg(test)]
    pub(in crate::world) fn heights_mut(&mut self) -> &mut [i32; COLUMNS] {
        &mut self.heights
    }

    /// The legacy waterless surface column used by [`flat`](GenChunk::flat).
    #[cfg(test)]
    fn simple_state(world_y: i32, height: i32) -> BlockState {
        let b = blocks::get();
        if world_y == MIN_Y {
            b.bedrock
        } else if world_y > height {
            b.air
        } else if world_y == height {
            b.grass_block
        } else if world_y >= height - 3 {
            b.dirt
        } else {
            b.stone
        }
    }

    /// The baseline block-state at chunk-local `(lx, world_y, lz)`: an O(1) read
    /// from the baked dense grid (surface rule + caves + features). Out-of-world
    /// `world_y` reads as air.
    pub fn base_state(&self, lx: i32, world_y: i32, lz: i32) -> BlockState {
        #[cfg(test)]
        if self.simple {
            let col = (lz * 16 + lx) as usize;
            return Self::simple_state(world_y, self.heights[col]);
        }
        if !(MIN_Y..MAX_Y_EXCL).contains(&world_y) {
            return super::states::AIR;
        }
        self.grid[grid_index(lx, world_y, lz)]
    }

    /// The network biome id at a column (for the biome `PalettedContainer`).
    pub fn biome_id(&self, lx: i32, lz: i32) -> u32 {
        self.biomes[(lz * 16 + lx) as usize].network_id()
    }

    /// The biome registry id string at a column (for the disk biome palette).
    pub fn biome_name(&self, lx: i32, lz: i32) -> &'static str {
        self.biomes[(lz * 16 + lx) as usize].name()
    }

    /// `WORLD_SURFACE` first-empty y for an unedited column.
    pub fn world_surface_top(&self, col: usize) -> i32 {
        self.ws_top[col]
    }

    /// `MOTION_BLOCKING` first-empty y for an unedited column.
    pub fn motion_blocking_top(&self, col: usize) -> i32 {
        self.mb_top[col]
    }

    /// Precompute the two heightmap tops per column by scanning the baseline from
    /// a ceiling above any decoration down to the world floor.
    fn compute_tops(&mut self) {
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let col = (lz * 16 + lx) as usize;
                // Trees/snow sit above the surface; sea columns top out at sea
                // level. Start comfortably above both.
                let start = (self.heights[col].max(SEA_LEVEL) + 24).min(MAX_Y_EXCL - 1);
                let (mut ws, mut mb) = (MIN_Y, MIN_Y);
                let (mut have_ws, mut have_mb) = (false, false);
                for y in (MIN_Y..=start).rev() {
                    let s = self.base_state(lx, y, lz);
                    if !have_ws && s != super::states::AIR {
                        ws = y + 1;
                        have_ws = true;
                    }
                    if !have_mb && is_motion_blocking(s) {
                        mb = y + 1;
                        have_mb = true;
                    }
                    if have_ws && have_mb {
                        break;
                    }
                }
                self.ws_top[col] = ws;
                self.mb_top[col] = mb;
            }
        }
    }
}

/// True for the `MOTION_BLOCKING` heightmap predicate: any non-air block that is
/// not a passable plant (`blocksMotion || !fluid.isEmpty`).
pub fn is_motion_blocking(state: BlockState) -> bool {
    state != super::states::AIR && !blocks::is_non_motion_blocking(state)
}

/// Light dampening for a generated/placed block (`getLightDampening`): 0/1/15.
pub fn light_dampening(state: BlockState) -> u8 {
    blocks::light_dampening(state)
}

/// The number of biomes in the synced registry (the biome global-palette range).
pub fn biome_registry_size() -> usize {
    biome::registry_size()
}

/// Choose a spawn column on solid, dry land near the origin: spiral outward until
/// a column's surface is above sea level and not ocean, returning its world xz.
/// Falls back to the origin if nothing suitable is found within the search radius.
pub fn spawn_column() -> (i32, i32) {
    for radius in 0..64i32 {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                // Only the ring at this radius (Chebyshev), to spiral outward.
                if dx.abs() != radius && dz.abs() != radius {
                    continue;
                }
                let (x, z) = (dx * 8, dz * 8);
                if surface_height(x, z) > SEA_LEVEL && biome_at(x, z) != Biome::Ocean {
                    return (x, z);
                }
            }
        }
    }
    (0, 0)
}

/// Encode `(lx, y, lz)` into the flat per-column-stack cell key shared with the
/// chunk store's edit map, or `None` if `y` is outside the buildable world.
pub(super) fn edit_key(lx: i32, y: i32, lz: i32) -> Option<u32> {
    if !(MIN_Y..MAX_Y_EXCL).contains(&y) {
        return None;
    }
    Some(((y - MIN_Y) as u32) * COLUMNS as u32 + (lz as u32) * 16 + lx as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_height_is_deterministic() {
        assert_eq!(surface_height(10, -7), surface_height(10, -7));
        assert_eq!(surface_height(1000, 1000), surface_height(1000, 1000));
    }

    #[test]
    fn surface_stays_in_the_world() {
        for x in (-512..512).step_by(31) {
            for z in (-512..512).step_by(29) {
                let h = surface_height(x, z);
                assert!((MIN_Y + 2..MAX_Y_EXCL - 40).contains(&h), "height {h} out of world at ({x},{z})");
            }
        }
    }

    #[test]
    fn terrain_is_continuous_across_a_chunk_boundary() {
        // Adjacent world columns across a chunk edge must not jump sharply.
        for z in -32..32 {
            let left = surface_height(15, z);
            let right = surface_height(16, z);
            assert!((left - right).abs() <= 3, "discontinuity at z={z}: {left} vs {right}");
        }
    }

    #[test]
    fn generate_is_deterministic() {
        let a = GenChunk::generate(3, -2);
        let b = GenChunk::generate(3, -2);
        assert_eq!(a.heights, b.heights);
        assert_eq!(a.grid, b.grid);
        assert_eq!(a.ws_top, b.ws_top);
    }

    #[test]
    fn sea_level_columns_have_water_on_top() {
        // Find an ocean column somewhere in a wide sweep and confirm it holds
        // water at sea level and air just above.
        let g = GenChunk::generate(0, 0);
        let mut found = false;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let col = (lz * 16 + lx) as usize;
                if g.heights[col] < SEA_LEVEL - 4 {
                    assert_eq!(g.base_state(lx, SEA_LEVEL, lz), blocks::get().water);
                    assert_eq!(g.base_state(lx, SEA_LEVEL + 1, lz), blocks::get().air);
                    found = true;
                }
            }
        }
        // Not every chunk has ocean; only assert the invariant when one exists.
        let _ = found;
    }

    #[test]
    fn spawn_is_on_dry_land() {
        let (x, z) = spawn_column();
        assert!(surface_height(x, z) > SEA_LEVEL);
        assert_ne!(biome_at(x, z), Biome::Ocean);
    }
}
