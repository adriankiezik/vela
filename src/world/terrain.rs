//! A small noise-based terrain generator: a hand-written value-noise heightmap
//! (a couple of fbm octaves), deterministic in world coordinates so it is
//! continuous across chunk boundaries.
//!
//! This is intentionally *not* a port of vanilla's `NoiseRouter`/`DensityFunction`
//! stack — just enough rolling terrain to stand on.

use super::{states, MIN_Y, SURFACE_Y};

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
pub(super) fn state_at(world_y: i32, height: i32) -> u32 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
