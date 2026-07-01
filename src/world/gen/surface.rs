//! The per-column surface rule: given a world cell, the column's surface height,
//! and its biome, decide the base block before decorations are overlaid.
//!
//! This is Vela's stand-in for vanilla's `SurfaceSystem` + `SurfaceRules`: a
//! ragged bedrock floor, a stone/deepslate fill carved by a simple 3D cave field,
//! a biome-appropriate surface + filler skin, sea-level water (with a frozen ice
//! cap and a thin snow layer in cold biomes), and a lava floor in the deepest
//! caves. Everything is a pure function of world coordinates + seed, so it is
//! seamless across chunk boundaries and reproducible for the persistence diff.

use crate::ids::BlockState;

use super::biome::{materials, Biome};
use super::blocks::get as blocks;
use super::noise::noise3;

/// Salts mixing the world seed into each independent field so they decorrelate.
const DEEPSLATE_SALT: u32 = 0x0DE5_1A7E;
const CAVE_SALT_A: u32 = 0xCA1E_0001;
const CAVE_SALT_B: u32 = 0xCA1E_0002;

/// Depth of the biome surface skin (top block + this many filler blocks).
const SKIN_DEPTH: i32 = 4;

/// A small integer hash → `[0, 1)` for the ragged bedrock/deepslate boundaries.
fn hash01(x: i32, y: i32, z: i32, seed: u32) -> f64 {
    let mut h = seed;
    h ^= (x as u32).wrapping_mul(0x9E37_79B1);
    h = h.wrapping_mul(0x85EB_CA77);
    h ^= (y as u32).wrapping_mul(0x68E3_1DA4);
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^= (z as u32).wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h as f64 / u32::MAX as f64
}

/// The ragged bedrock floor: solid at the world floor, thinning out over the four
/// layers above (`Mth`-style probabilistic bedrock, like vanilla's floor).
fn is_bedrock(wx: i32, wy: i32, wz: i32, min_y: i32, seed: u32) -> bool {
    if wy == min_y {
        return true;
    }
    let layer = wy - min_y; // 1..=4 in the ragged band
    if !(1..5).contains(&layer) {
        return false;
    }
    // Chance decreases with height: layer 1 → 4/5, layer 4 → 1/5.
    hash01(wx, wy, wz, seed ^ 0xB3D0_0C4E) < (5 - layer) as f64 / 5.0
}

/// The stone→deepslate transition: stone at/above y=0, deepslate at/below y=-8,
/// a ragged probabilistic border between (mirrors vanilla's deepslate blending).
fn is_deepslate(wx: i32, wy: i32, wz: i32, seed: u32) -> bool {
    if wy >= 0 {
        return false;
    }
    if wy <= -8 {
        return true;
    }
    let t = (-wy) as f64 / 8.0; // 0..1, deepslate probability rising with depth
    hash01(wx, wy, wz, seed ^ DEEPSLATE_SALT) < t
}

/// A simple two-field cave carve: where two independent 3D noise iso-surfaces both
/// pass near zero they intersect in tube-like tunnels. Kept a few blocks below the
/// surface skin and above the bedrock band so it never breaches the top block
/// (which would desync the short-circuited heightmap).
fn is_cave(wx: i32, wy: i32, wz: i32, seed: u32) -> bool {
    const S: f64 = 1.0 / 22.0;
    let (fx, fy, fz) = (wx as f64 * S, wy as f64 * S * 1.7, wz as f64 * S);
    let a = noise3(fx, fy, fz, seed ^ CAVE_SALT_A);
    let b = noise3(fx, fy, fz, seed ^ CAVE_SALT_B);
    a.abs() < 0.08 && b.abs() < 0.08
}

/// The base block-state at world `(wx, wy, wz)` in a column whose solid surface is
/// at `height`, for `biome`, with the given `sea_level` and world floor `min_y`.
pub fn column_state(
    wx: i32,
    wy: i32,
    wz: i32,
    height: i32,
    biome: Biome,
    sea_level: i32,
    min_y: i32,
    seed: u32,
) -> BlockState {
    let b = blocks();

    if is_bedrock(wx, wy, wz, min_y, seed) {
        return b.bedrock;
    }

    // Above the solid surface: air, sea-level water, or a cold cap.
    if wy > height {
        if wy <= sea_level {
            if biome.is_snowy() && wy == sea_level {
                return b.ice; // frozen surface layer over the water below
            }
            return b.water;
        }
        if biome.is_snowy() && wy == height + 1 && height >= sea_level {
            return b.snow_layer; // thin snow on exposed cold ground
        }
        return b.air;
    }

    // Below the surface skin, carve caves (with a deep lava floor).
    let lava_level = min_y + 8;
    if wy < height - (SKIN_DEPTH - 1)
        && wy > min_y + 4
        && is_cave(wx, wy, wz, seed)
    {
        return if wy <= lava_level { b.lava } else { b.air };
    }

    let (top, filler, underwater) = materials(biome);

    if wy == height {
        return if height < sea_level { underwater } else { top };
    }
    if wy > height - SKIN_DEPTH {
        // Under-sand columns get sand filler already via the biome; land columns
        // get dirt/stone filler.
        return filler;
    }

    if is_deepslate(wx, wy, wz, seed) {
        b.deepslate
    } else {
        b.stone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEA: i32 = 63;
    const MIN_Y: i32 = -64;
    const SEED: u32 = 0x5EED_C0DE;

    #[test]
    fn floor_is_bedrock_and_top_is_grass() {
        let bl = blocks();
        assert_eq!(
            column_state(0, MIN_Y, 0, 70, Biome::Plains, SEA, MIN_Y, SEED),
            bl.bedrock
        );
        assert_eq!(
            column_state(0, 70, 0, 70, Biome::Plains, SEA, MIN_Y, SEED),
            bl.grass_block
        );
        assert_eq!(
            column_state(0, 71, 0, 70, Biome::Plains, SEA, MIN_Y, SEED),
            bl.air
        );
    }

    #[test]
    fn ocean_columns_fill_with_water_to_sea_level() {
        let bl = blocks();
        // Ocean floor at y=50, sea level 63: y=60 must be water, y=64 air.
        assert_eq!(
            column_state(4, 60, 4, 50, Biome::Ocean, SEA, MIN_Y, SEED),
            bl.water
        );
        assert_eq!(
            column_state(4, 64, 4, 50, Biome::Ocean, SEA, MIN_Y, SEED),
            bl.air
        );
        // The floor block itself is the biome underwater material (sand).
        assert_eq!(
            column_state(4, 50, 4, 50, Biome::Ocean, SEA, MIN_Y, SEED),
            bl.sand
        );
    }

    #[test]
    fn desert_surface_is_sand() {
        let bl = blocks();
        assert_eq!(
            column_state(9, 72, 9, 72, Biome::Desert, SEA, MIN_Y, SEED),
            bl.sand
        );
    }

    #[test]
    fn deep_fill_below_zero_is_deepslate_dominant() {
        let bl = blocks();
        // Well below y=-8 the fill must be deepslate (unless carved to a cave).
        let s = column_state(3, -40, 3, 70, Biome::Plains, SEA, MIN_Y, SEED);
        assert!(s == bl.deepslate || s == bl.air || s == bl.lava);
    }
}
