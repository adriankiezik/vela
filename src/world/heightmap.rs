//! The two client-facing heightmaps (`WORLD_SURFACE`, `MOTION_BLOCKING`) derived
//! from a chunk's generated heights plus any per-cell edits.

use std::collections::{HashMap, HashSet};

use super::bitpack::pack_bits;
use super::chunk_data::cell_state;
use super::{states, COLUMNS, MAX_Y_EXCL, MIN_Y, WORLD_HEIGHT};

/// The two client-facing heightmaps (`WORLD_SURFACE` = id 1, `MOTION_BLOCKING`
/// = id 4) for a chunk, each a packed `long[]` of 256 column heights. With no
/// water or non-occluding cover both equal the first free y above the highest
/// non-air block, relative to the world floor (`firstAvailable - minY`).
///
/// Edits are folded in by recomputing each column's top non-air block: an
/// unedited column short-circuits to its generated surface height, while an
/// edited column is scanned from the top so a placed block raises the map and a
/// broken surface lowers it.
pub(super) fn compute_heightmaps(
    heights: &[i32; COLUMNS],
    edits: &HashMap<u32, u32>,
) -> Vec<(i32, Vec<i64>)> {
    // Bits = ceil(log2(worldHeight + 1)); a 384-tall column -> 9.
    let bits = ((WORLD_HEIGHT + 1) as u32)
        .next_power_of_two()
        .trailing_zeros();
    // Precompute the set of columns that carry at least one edit (O(edits)) so
    // each column's heightmap lookup is O(1) instead of rescanning every edit.
    let mut edited_cols: HashSet<u32> = HashSet::new();
    for &k in edits.keys() {
        edited_cols.insert(k % COLUMNS as u32);
    }
    let mut values = [0u64; COLUMNS];
    for lz in 0..16i32 {
        for lx in 0..16i32 {
            let col = (lz * 16 + lx) as usize;
            values[col] =
                (column_first_empty(heights, edits, &edited_cols, lx, lz) - MIN_Y) as u64;
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
    edited_cols: &HashSet<u32>,
    lx: i32,
    lz: i32,
) -> i32 {
    let height = heights[(lz * 16 + lx) as usize];
    if !edited_cols.contains(&((lz as u32) * 16 + lx as u32)) {
        return height + 1;
    }
    for y in (MIN_Y..MAX_Y_EXCL).rev() {
        if cell_state(heights, edits, lx, y, lz) != states::AIR {
            return y + 1;
        }
    }
    MIN_Y
}

#[cfg(test)]
mod tests {
    //! Parity tests for the client-facing heightmaps against vanilla
    //! `Heightmap` (MC 26.2): both `WORLD_SURFACE` (id 1) and `MOTION_BLOCKING`
    //! (id 4) store `firstAvailable(x,z) - minY` per column, packed with
    //! `SimpleBitStorage` at `ceil(log2(worldHeight+1))` bits.

    use super::*;
    use crate::world::{SECTION_COUNT, WORLD_HEIGHT};

    // 9 bits/entry for a 384-tall world -> 7 values per long -> 37 longs / 256.
    const BITS: u32 = 9;
    const PER_LONG: usize = 64 / BITS as usize;
    const EXPECTED_LONGS: usize = COLUMNS.div_ceil(PER_LONG);

    /// The stored heightmap value for column `(lx, lz)`, unpacked from the
    /// non-spanning `SimpleBitStorage` layout.
    fn stored(packed: &[i64], lx: i32, lz: i32) -> u64 {
        let col = (lz * 16 + lx) as usize;
        let raw = packed[col / PER_LONG] as u64;
        (raw >> ((col % PER_LONG) * BITS as usize)) & ((1 << BITS) - 1)
    }

    /// The same flat edit key `chunk_data::edit_key` builds, so tests can inject
    /// per-cell overrides without going through `ChunkData`.
    fn key(lx: i32, y: i32, lz: i32) -> u32 {
        ((y - MIN_Y) as u32) * COLUMNS as u32 + (lz as u32) * 16 + lx as u32
    }

    fn flat_heights(h: i32) -> [i32; COLUMNS] {
        [h; COLUMNS]
    }

    #[test]
    fn bit_width_is_nine_for_384_tall_world() {
        assert_eq!(WORLD_HEIGHT, SECTION_COUNT * 16);
        let bits = ((WORLD_HEIGHT + 1) as u32).next_power_of_two().trailing_zeros();
        assert_eq!(bits, BITS, "ceil(log2(385)) == 9");
    }

    #[test]
    fn unedited_flat_surface_stores_height_plus_one_minus_floor() {
        // With no cover, firstAvailable = height + 1, stored relative to minY.
        let heights = flat_heights(63);
        let maps = compute_heightmaps(&heights, &HashMap::new());

        // Two maps: WORLD_SURFACE (1) and MOTION_BLOCKING (4), identical data.
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].0, 1);
        assert_eq!(maps[1].0, 4);
        assert_eq!(maps[0].1, maps[1].1);
        assert_eq!(maps[0].1.len(), EXPECTED_LONGS);

        let expected = (63 + 1 - MIN_Y) as u64; // 128
        assert_eq!(stored(&maps[0].1, 0, 0), expected);
        assert_eq!(stored(&maps[0].1, 15, 15), expected);
        assert_eq!(stored(&maps[0].1, 7, 3), expected);
    }

    #[test]
    fn placing_a_block_above_the_surface_raises_the_column() {
        // A stone placed at y=70 over a surface-63 column makes it the top
        // non-air block -> firstAvailable = 71.
        let heights = flat_heights(63);
        let mut edits = HashMap::new();
        edits.insert(key(0, 70, 0), states::STONE);

        let maps = compute_heightmaps(&heights, &edits);

        assert_eq!(stored(&maps[0].1, 0, 0), (71 - MIN_Y) as u64); // raised
        assert_eq!(stored(&maps[0].1, 1, 0), (64 - MIN_Y) as u64); // neighbour unchanged
    }

    #[test]
    fn breaking_the_surface_block_lowers_the_column() {
        // Removing the grass at y=63 exposes the dirt at y=62 (state_at: dirt
        // within height-3) -> firstAvailable = 63, one lower than the baseline.
        let heights = flat_heights(63);
        let mut edits = HashMap::new();
        edits.insert(key(0, 63, 0), states::AIR);

        let maps = compute_heightmaps(&heights, &edits);

        assert_eq!(stored(&maps[0].1, 0, 0), (63 - MIN_Y) as u64); // lowered by 1
        assert_eq!(stored(&maps[0].1, 1, 0), (64 - MIN_Y) as u64); // neighbour unchanged
    }

    #[test]
    fn fully_air_column_stores_zero() {
        // A column whose surface is below the floor generates only the y=minY
        // bedrock cell; dig that out too and the whole column is air, so
        // firstAvailable == minY -> stored 0. Exercises the `MIN_Y` return path.
        let mut heights = flat_heights(63);
        let col = (0 * 16 + 0) as usize;
        heights[col] = MIN_Y - 1; // surface below the world floor
        let mut edits = HashMap::new();
        edits.insert(key(0, MIN_Y, 0), states::AIR); // remove the bedrock cell

        let maps = compute_heightmaps(&heights, &edits);

        assert_eq!(stored(&maps[0].1, 0, 0), 0);
    }
}
