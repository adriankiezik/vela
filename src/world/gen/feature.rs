//! The per-chunk decoration pass: ores in the fill, trees, ground plants, and the
//! odd desert cactus — produced as a deterministic map of block-state overrides
//! keyed the same way the chunk store keys player edits.
//!
//! This mirrors the *shape* of vanilla's `PlacedFeature`/`ConfiguredFeature`
//! decoration (ore blobs, trees, grass/flower patches) driven off a
//! `WorldgenRandom` seeded per chunk from the level seed and the chunk origin
//! (`setDecorationSeed`). Because the whole pass is a pure function of
//! `(seed, cx, cz)` it reproduces exactly on reload, which the persistence diff
//! (regenerate baseline, diff against saved blocks) depends on.
//!
//! Simplification vs vanilla: features are confined to their origin chunk — tree
//! trunks are only rooted where the whole canopy fits inside the chunk, so nothing
//! is written into a neighbour. Cross-chunk feature bleed is deferred (noted in the
//! roadmap), which keeps generation embarrassingly parallel and seam-free.

use std::collections::HashMap;

use crate::ids::BlockState;

use super::biome::{Biome, TreeKind};
use super::blocks::{get as blocks, Blocks};
use super::edit_key;
use super::rng::JavaRandom;
use super::super::COLUMNS;

/// One ore's placement parameters: the stone- and deepslate-hosted variants, how
/// many veins to try per chunk, the vein size, and the inclusive world-y band.
struct Ore {
    stone: fn(&Blocks) -> BlockState,
    deep: fn(&Blocks) -> BlockState,
    count: i32,
    size: i32,
    min_y: i32,
    max_y: i32,
}

/// The ore table, roughly echoing vanilla's overworld ore distribution (counts
/// and heights are approximate, not the exact `OreConfiguration` triangles).
const ORES: &[Ore] = &[
    Ore { stone: |b| b.coal_ore,     deep: |b| b.deepslate_coal_ore,     count: 20, size: 8, min_y: 0,   max_y: 190 },
    Ore { stone: |b| b.iron_ore,     deep: |b| b.deepslate_iron_ore,     count: 20, size: 6, min_y: -24, max_y: 56  },
    Ore { stone: |b| b.copper_ore,   deep: |b| b.deepslate_copper_ore,   count: 16, size: 6, min_y: -16, max_y: 112 },
    Ore { stone: |b| b.gold_ore,     deep: |b| b.deepslate_gold_ore,     count: 4,  size: 6, min_y: -64, max_y: 32  },
    Ore { stone: |b| b.redstone_ore, deep: |b| b.deepslate_redstone_ore, count: 8,  size: 6, min_y: -64, max_y: 15  },
    Ore { stone: |b| b.lapis_ore,    deep: |b| b.deepslate_lapis_ore,    count: 2,  size: 5, min_y: -32, max_y: 32  },
    Ore { stone: |b| b.diamond_ore,  deep: |b| b.deepslate_diamond_ore,  count: 2,  size: 4, min_y: -63, max_y: 16  },
    Ore { stone: |b| b.emerald_ore,  deep: |b| b.deepslate_emerald_ore,  count: 3,  size: 3, min_y: -16, max_y: 120 },
    // Stone blobs (share the ore-blob machinery, no deepslate variant of their own
    // that matters here — tuff stands in below zero).
    Ore { stone: |b| b.granite,  deep: |b| b.granite,  count: 8, size: 12, min_y: 0,   max_y: 90 },
    Ore { stone: |b| b.diorite,  deep: |b| b.diorite,  count: 8, size: 12, min_y: 0,   max_y: 90 },
    Ore { stone: |b| b.andesite, deep: |b| b.andesite, count: 8, size: 12, min_y: 0,   max_y: 90 },
    Ore { stone: |b| b.dirt,     deep: |b| b.tuff,     count: 6, size: 10, min_y: -20, max_y: 80 },
];

/// Decorate chunk `(cx, cz)` from its column heights and biomes, returning the
/// map of generated block-state overrides (keyed by [`edit_key`]).
pub fn decorate(
    cx: i32,
    cz: i32,
    heights: &[i32; COLUMNS],
    biomes: &[Biome; COLUMNS],
    sea_level: i32,
    seed: u64,
) -> HashMap<u32, BlockState> {
    let b = blocks();
    let mut out: HashMap<u32, BlockState> = HashMap::new();
    let ox = cx * 16;
    let oz = cz * 16;

    let mut rng = JavaRandom::new(0);
    rng.set_decoration_seed(seed as i64, ox, oz);

    place_ores(&mut out, heights, &mut rng, b);
    place_trees(&mut out, heights, biomes, sea_level, &mut rng, b);
    place_plants(&mut out, heights, biomes, sea_level, &mut rng, b);

    out
}

/// Overwrite the cell at local `(lx, wy, lz)` with `state` (used for solid
/// features that should win over the terrain skin).
fn put(out: &mut HashMap<u32, BlockState>, lx: i32, wy: i32, lz: i32, state: BlockState) {
    if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
        return;
    }
    if let Some(k) = edit_key(lx, wy, lz) {
        out.insert(k, state);
    }
}

/// Place the cell only if nothing has claimed it yet (used for plants, so they
/// never punch through a trunk or a neighbouring feature).
fn put_soft(out: &mut HashMap<u32, BlockState>, lx: i32, wy: i32, lz: i32, state: BlockState) {
    if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
        return;
    }
    if let Some(k) = edit_key(lx, wy, lz) {
        out.entry(k).or_insert(state);
    }
}

/// The column height at chunk-local `(lx, lz)`.
fn col_height(heights: &[i32; COLUMNS], lx: i32, lz: i32) -> i32 {
    heights[(lz * 16 + lx) as usize]
}

/// The biome at chunk-local `(lx, lz)`.
fn col_biome(biomes: &[Biome; COLUMNS], lx: i32, lz: i32) -> Biome {
    biomes[(lz * 16 + lx) as usize]
}

/// Scatter ore/stone blobs through the fill.
fn place_ores(
    out: &mut HashMap<u32, BlockState>,
    heights: &[i32; COLUMNS],
    rng: &mut JavaRandom,
    b: &Blocks,
) {
    for ore in ORES {
        for _ in 0..ore.count {
            let lx = rng.next_int(16);
            let lz = rng.next_int(16);
            let span = (ore.max_y - ore.min_y).max(1);
            let cy = ore.min_y + rng.next_int(span);
            // A compact blob around the seed cell.
            for _ in 0..ore.size {
                let dx = rng.next_int(3) - 1;
                let dy = rng.next_int(3) - 1;
                let dz = rng.next_int(3) - 1;
                let (bx, by, bz) = (lx + dx, cy + dy, lz + dz);
                if !(0..16).contains(&bx) || !(0..16).contains(&bz) {
                    continue;
                }
                // Keep ore inside the solid fill (below the surface skin), so it
                // never floats in air or water.
                if by >= col_height(heights, bx, bz) - 1 {
                    continue;
                }
                let state = if by < 0 { (ore.deep)(b) } else { (ore.stone)(b) };
                put(out, bx, by, bz, state);
            }
        }
    }
}

/// Plant trees according to each column's biome, rooted only where the canopy fits
/// inside the chunk.
fn place_trees(
    out: &mut HashMap<u32, BlockState>,
    heights: &[i32; COLUMNS],
    biomes: &[Biome; COLUMNS],
    sea_level: i32,
    rng: &mut JavaRandom,
    b: &Blocks,
) {
    // Tree budget from the chunk-centre biome (a whole chunk is one biome most of
    // the time; borders just try the centre's count).
    let count = col_biome(biomes, 8, 8).trees().1;
    for _ in 0..count {
        // Root within [2, 13] so a radius-2 canopy stays inside the chunk.
        let lx = 2 + rng.next_int(12);
        let lz = 2 + rng.next_int(12);
        let biome = col_biome(biomes, lx, lz);
        let (kind, _) = biome.trees();
        if kind == TreeKind::None {
            continue;
        }
        let ground = col_height(heights, lx, lz);
        // Only on dry land above sea level, and not right at the build ceiling.
        if ground < sea_level || ground > 240 {
            continue;
        }
        build_tree(out, lx, lz, ground, kind, rng, b);
    }
}

/// Build one tree: a log trunk topped with a species-coloured leaf canopy.
fn build_tree(
    out: &mut HashMap<u32, BlockState>,
    lx: i32,
    lz: i32,
    ground: i32,
    kind: TreeKind,
    rng: &mut JavaRandom,
    b: &Blocks,
) {
    let (log, leaf, trunk_h) = match kind {
        TreeKind::Oak => (b.oak_log, b.oak_leaves, 4 + rng.next_int(3)),
        TreeKind::Birch => (b.birch_log, b.birch_leaves, 5 + rng.next_int(3)),
        TreeKind::Spruce => (b.spruce_log, b.spruce_leaves, 6 + rng.next_int(4)),
        TreeKind::Jungle => (b.jungle_log, b.jungle_leaves, 6 + rng.next_int(6)),
        TreeKind::Acacia => (b.acacia_log, b.acacia_leaves, 5 + rng.next_int(2)),
        TreeKind::DarkOak => (b.dark_oak_log, b.dark_oak_leaves, 6 + rng.next_int(2)),
        TreeKind::None => return,
    };
    let base = ground + 1; // first log sits on the surface block
    let top = base + trunk_h; // leaf-centre height

    // Trunk.
    for y in base..top {
        put(out, lx, y, lz, log);
    }

    // Canopy: two wide layers (radius 2) then two narrow (radius 1), with the
    // radius-2 corners trimmed for a rounded look.
    for (dy, radius) in [(-2i32, 2i32), (-1, 2), (0, 1), (1, 1)] {
        let y = top + dy;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx == 0 && dz == 0 && dy <= 0 {
                    continue; // trunk occupies the centre of the lower layers
                }
                if radius == 2 && dx.abs() == 2 && dz.abs() == 2 && rng.next_int(2) == 0 {
                    continue; // trim a corner
                }
                put_soft(out, lx + dx, y, lz + dz, leaf);
            }
        }
    }
}

/// Scatter ground cover: grass tufts, ferns, flowers, and desert flora.
fn place_plants(
    out: &mut HashMap<u32, BlockState>,
    heights: &[i32; COLUMNS],
    biomes: &[Biome; COLUMNS],
    sea_level: i32,
    rng: &mut JavaRandom,
    b: &Blocks,
) {
    let center = col_biome(biomes, 8, 8);

    // Grass / fern tufts.
    for _ in 0..center.grass_tufts() {
        let lx = rng.next_int(16);
        let lz = rng.next_int(16);
        let biome = col_biome(biomes, lx, lz);
        let ground = col_height(heights, lx, lz);
        if ground < sea_level || biome.is_snowy() {
            continue;
        }
        let tuft = match biome {
            Biome::Taiga | Biome::SnowyTaiga => b.fern,
            Biome::Desert | Biome::Beach | Biome::StonyShore => b.dead_bush,
            _ => b.short_grass,
        };
        put_soft(out, lx, ground + 1, lz, tuft);
    }

    // Flowers.
    for _ in 0..center.flowers() {
        let lx = rng.next_int(16);
        let lz = rng.next_int(16);
        let biome = col_biome(biomes, lx, lz);
        let ground = col_height(heights, lx, lz);
        if ground < sea_level || biome.is_snowy() || matches!(biome, Biome::Desert | Biome::Ocean) {
            continue;
        }
        let flower = match rng.next_int(5) {
            0 => b.dandelion,
            1 => b.poppy,
            2 => b.cornflower,
            3 => b.oxeye_daisy,
            _ => b.azure_bluet,
        };
        put_soft(out, lx, ground + 1, lz, flower);
    }

    // Desert cacti (1–3 tall) and the odd swamp/forest mushroom.
    if center == Biome::Desert {
        for _ in 0..rng.next_int(6) {
            let lx = 1 + rng.next_int(14);
            let lz = 1 + rng.next_int(14);
            let ground = col_height(heights, lx, lz);
            if ground < sea_level {
                continue;
            }
            let tall = 1 + rng.next_int(3);
            for i in 0..tall {
                put_soft(out, lx, ground + 1 + i, lz, b.cactus);
            }
        }
    } else if matches!(center, Biome::Swamp | Biome::DarkForest) {
        for _ in 0..rng.next_int(4) {
            let lx = rng.next_int(16);
            let lz = rng.next_int(16);
            let ground = col_height(heights, lx, lz);
            if ground < sea_level {
                continue;
            }
            let shroom = if rng.next_int(2) == 0 { b.brown_mushroom } else { b.red_mushroom };
            put_soft(out, lx, ground + 1, lz, shroom);
        }
    }
}
