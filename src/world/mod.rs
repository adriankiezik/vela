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

use crate::protocol::buffer::PacketWriter;

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
    let n = fbm(world_x as f64 / TERRAIN_SCALE, world_z as f64 / TERRAIN_SCALE);
    let h = SURFACE_Y as f64 + n * TERRAIN_AMPLITUDE;
    (h.round() as i32).clamp(HEIGHT_MIN, HEIGHT_MAX)
}

/// The block state at `world_y` in a column whose surface is at `height`:
/// bedrock floor, stone fill, three dirt layers under the surface, a grass
/// block on top, air above.
fn state_at(world_y: i32, height: i32) -> u32 {
    if world_y > height {
        states::AIR
    } else if world_y == height {
        states::GRASS_BLOCK
    } else if world_y >= height - 3 {
        states::DIRT
    } else if world_y == MIN_Y {
        states::BEDROCK
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

/// The 24-section block blob for a specific chunk `(cx, cz)`. Each of the 256
/// columns may have a different height, so this is computed per chunk (owned).
pub fn column_blob(cx: i32, cz: i32) -> Vec<u8> {
    let heights = chunk_heights(cx, cz);
    let mut out = PacketWriter::new();
    for section in 0..SECTION_COUNT {
        let base_y = MIN_Y + section * 16;
        encode_section(base_y, &heights, &mut out);
    }
    out.buf.to_vec()
}

/// Serialize one section: counts, then the block-state and biome containers.
/// `heights` are the 256 per-column surface heights for this chunk, indexed
/// `lz * 16 + lx`.
fn encode_section(base_y: i32, heights: &[i32; COLUMNS], out: &mut PacketWriter) {
    // Cell index is vanilla's `(y << 8) | (z << 4) | x`.
    let mut cells = [states::AIR; CELLS];
    let mut non_air: u16 = 0;
    for ly in 0..16i32 {
        let world_y = base_y + ly;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let height = heights[(lz * 16 + lx) as usize];
                let state = state_at(world_y, height);
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
    write_single_value(states::AIR /* biome id 0 == registry index 0 */, out);
}

/// Write a block-state `PalettedContainer`. A uniform section collapses to a
/// single-value palette (0 bits, no data array); otherwise we use a 4-bit
/// linear palette — vanilla pads palettes of 1–4 bits up to 4 for block states.
///
/// Our terrain uses at most five distinct states per section
/// (air/grass/dirt/stone/bedrock), well under the 16-entry (4-bit) ceiling, so
/// the fixed 4-bit width always suffices; a wider section would need a wider
/// `BITS` and is rejected by the debug assert below.
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

    const BITS: u32 = 4;
    // 4-bit linear palette caps at 16 entries; extend BITS before exceeding it.
    debug_assert!(
        palette.len() <= 16,
        "section palette {} exceeds 4-bit linear capacity",
        palette.len()
    );
    out.write_u8(BITS as u8);
    out.write_varint(palette.len() as i32);
    for &state in &palette {
        out.write_varint(state as i32);
    }
    let indices: Vec<u64> = cells
        .iter()
        .map(|c| palette.iter().position(|p| p == c).unwrap() as u64)
        .collect();
    for long in pack_bits(&indices, BITS) {
        out.write_i64(long as i64);
    }
}

/// A single-value (0-bit) `PalettedContainer`: just the value, no storage.
fn write_single_value(value: u32, out: &mut PacketWriter) {
    out.write_u8(0); // bits per entry
    out.write_varint(value as i32); // the sole palette entry
    // No data array follows a 0-bit storage.
}

/// The two client-facing heightmaps (`WORLD_SURFACE` = id 1, `MOTION_BLOCKING`
/// = id 4) for chunk `(cx, cz)`, each a packed `long[]` of 256 column heights.
/// With no water or non-occluding cover both equal the first free y above the
/// surface, relative to the world floor (`firstAvailable - minY`).
pub fn heightmaps(cx: i32, cz: i32) -> Vec<(i32, Vec<i64>)> {
    let heights = chunk_heights(cx, cz);
    // Bits = ceil(log2(worldHeight + 1)); a 384-tall column -> 9.
    let bits = ((SECTION_COUNT * 16 + 1) as u32)
        .next_power_of_two()
        .trailing_zeros();
    let values: Vec<u64> = heights
        .iter()
        .map(|&h| (h + 1 - MIN_Y) as u64) // first empty y above the surface
        .collect();
    let packed: Vec<i64> = pack_bits(&values, bits)
        .into_iter()
        .map(|l| l as i64)
        .collect();
    vec![(1, packed.clone()), (4, packed)]
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
        let maps = heightmaps(0, 0);
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
        let a = column_blob(0, 0);
        assert!(a.len() > (SECTION_COUNT as usize) * 8);
        // Distant chunks have different terrain, hence different bytes.
        let b = column_blob(50, 50);
        assert_ne!(a, b);
    }

    #[test]
    fn column_blob_is_deterministic() {
        assert_eq!(column_blob(2, 5), column_blob(2, 5));
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
}

