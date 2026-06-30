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
