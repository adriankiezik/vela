//! Chunk ↔ Anvil NBT (de)serialization.
//!
//! Reference: decompiled `SerializableChunkData` and `PalettedContainer`'s NBT
//! codec (MC 26.2). We serialize a chunk's 24 sections as vanilla does — a
//! `sections` list of `{Y, block_states, biomes}` compounds, a `Heightmaps`
//! compound, and the bookkeeping fields — and parse the block palettes back on
//! load. This is also the **storage read path for foreign paletted containers**
//! the roadmap flags: a `{palette, data}` block-state container is decoded into
//! flat global state ids regardless of which palette width produced it.
//!
//! The disk paletted-container format differs from the network one:
//!   * the palette lists **named** entries (`{Name, Properties}` for blocks, a
//!     bare id string for biomes), not numeric global ids;
//!   * there is no leading bits-per-entry byte — the storage width is recomputed
//!     from the palette size (`Strategy.getConfigurationForPaletteSize`); and
//!   * a single-entry palette omits the `data` array entirely.
//!
//! Vela stores block state only. Biomes are written as a single representative
//! (chunk-centre) biome per section and ignored on read — the chunk store
//! re-derives per-column biomes deterministically from the generator — and
//! lighting, block entities, ticks, and structures are written empty.

use std::collections::HashMap;

use crate::ids::BlockState;
use crate::protocol::nbt::Nbt;
use crate::registry::block_state::{describe_state, with_properties};

use super::super::bitpack::{pack_bits, unpack_bits};
use super::super::chunk_data::cell_state;
use super::super::gen::GenChunk;
use super::super::heightmap::compute_heightmaps;
use super::super::{states, CELLS, MIN_Y, SECTION_COUNT};

/// Current world data version (`SharedConstants.WORLD_VERSION`, MC 26.2). Written
/// as `DataVersion` so a vanilla client/server recognises the save format.
const DATA_VERSION: i32 = 4903;

/// The lowest section's Y coordinate (`minSectionY = MIN_Y / 16`), written as
/// `yPos` and used to offset each section's `Y` byte.
const MIN_SECTION_Y: i32 = MIN_Y / 16;

/// Serialize chunk `(cx, cz)` — given its generated column heights and per-cell
/// edits — into an Anvil chunk NBT compound ready for the region file. Mirrors
/// `SerializableChunkData.write` for a fully-generated (`minecraft:full`) chunk.
pub fn to_nbt(
    cx: i32,
    cz: i32,
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
    game_time: i64,
) -> Nbt {
    let mut sections: Vec<Nbt> = Vec::with_capacity(SECTION_COUNT as usize);
    for section in 0..SECTION_COUNT {
        let base_y = MIN_Y + section * 16;
        let y_byte = (MIN_SECTION_Y + section) as i8;
        sections.push(section_nbt(y_byte, base_y, gen, edits));
    }

    let maps = compute_heightmaps(gen, edits);
    let heightmaps = Nbt::compound(
        maps.into_iter()
            .map(|(id, longs)| (heightmap_key(id).to_string(), Nbt::LongArray(longs))),
    );

    Nbt::compound([
        ("DataVersion", Nbt::Int(DATA_VERSION)),
        ("xPos", Nbt::Int(cx)),
        ("yPos", Nbt::Int(MIN_SECTION_Y)),
        ("zPos", Nbt::Int(cz)),
        ("LastUpdate", Nbt::Long(game_time)),
        ("InhabitedTime", Nbt::Long(0)),
        ("Status", Nbt::string("minecraft:full")),
        ("sections", Nbt::List(sections)),
        ("Heightmaps", heightmaps),
        ("block_entities", Nbt::List(vec![])),
        ("block_ticks", Nbt::List(vec![])),
        ("fluid_ticks", Nbt::List(vec![])),
        ("PostProcessing", Nbt::List(vec![])),
        ("structures", Nbt::compound([
            ("starts", Nbt::Compound(vec![])),
            ("References", Nbt::Compound(vec![])),
        ])),
    ])
}

/// Parse an Anvil chunk NBT compound back into a dense grid of block-state ids,
/// indexed `section * CELLS + (ly << 8 | lz << 4 | lx)` — the same order
/// [`to_nbt`] wrote. Returns `None` if the tag is not a readable full chunk.
///
/// Sections absent from the `sections` list (or whose `Y` falls outside the
/// buildable range) read as all-air. This is the paletted-container read path:
/// each section's `block_states` `{palette, data}` is decoded independently of
/// the width vanilla chose.
pub fn from_nbt(nbt: &Nbt) -> Option<Vec<BlockState>> {
    // A chunk with no status was never generated; reject it like vanilla `parse`.
    if nbt.get("Status").and_then(Nbt::as_str)?.is_empty() {
        return None;
    }
    let mut grid = vec![states::AIR; (SECTION_COUNT as usize) * CELLS];

    if let Some(Nbt::List(sections)) = nbt.get("sections") {
        for section in sections {
            let y = match section.get("Y") {
                Some(Nbt::Byte(y)) => *y as i32,
                _ => continue,
            };
            let idx = y - MIN_SECTION_Y;
            if !(0..SECTION_COUNT).contains(&idx) {
                continue; // outside our column height — ignore
            }
            let Some(block_states) = section.get("block_states") else {
                continue; // absent container ⇒ all air (grid already air)
            };
            let cells = decode_block_states(block_states)?;
            let base = (idx as usize) * CELLS;
            grid[base..base + CELLS].copy_from_slice(&cells);
        }
    }

    Some(grid)
}

/// Build one section compound: `Y`, the block-state container, and the
/// single-value biome container. A section is always written (never elided) so
/// the on-disk section count matches the column height.
fn section_nbt(
    y_byte: i8,
    base_y: i32,
    gen: &GenChunk,
    edits: &HashMap<u32, BlockState>,
) -> Nbt {
    let mut cells = [states::AIR; CELLS];
    for ly in 0..16i32 {
        let world_y = base_y + ly;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let idx = ((ly << 8) | (lz << 4) | lx) as usize;
                cells[idx] = cell_state(gen, edits, lx, world_y, lz);
            }
        }
    }

    Nbt::compound([
        ("Y", Nbt::Byte(y_byte)),
        ("block_states", encode_block_states(&cells)),
        ("biomes", single_value_biomes(gen)),
    ])
}

/// Encode a section's 4096 block-state ids as the NBT `{palette, data}` block
/// container. A uniform section stores just its one-entry palette (no `data`);
/// otherwise the indices are packed at the width vanilla derives from the palette
/// size (`max(4, ceil(log2(size)))`).
fn encode_block_states(cells: &[BlockState; CELLS]) -> Nbt {
    // First-seen distinct states (sections are near-uniform, so a linear scan
    // beats a hashed set here, as in the network encoder).
    let mut palette: Vec<BlockState> = Vec::new();
    for &c in cells.iter() {
        if !palette.contains(&c) {
            palette.push(c);
        }
    }

    let palette_tags: Vec<Nbt> = palette.iter().map(|&s| block_palette_entry(s.get())).collect();

    if palette.len() == 1 {
        // Single value: palette only, no storage array (ZeroBitStorage).
        return Nbt::compound([("palette", Nbt::List(palette_tags))]);
    }

    let bits = block_storage_bits(palette.len());
    let indices: Vec<u64> = cells
        .iter()
        .map(|c| palette.iter().position(|p| p == c).unwrap() as u64)
        .collect();
    let data: Vec<i64> = pack_bits(&indices, bits).into_iter().map(|l| l as i64).collect();
    Nbt::compound([
        ("palette", Nbt::List(palette_tags)),
        ("data", Nbt::LongArray(data)),
    ])
}

/// Decode an NBT `block_states` container back into 4096 global state ids.
fn decode_block_states(container: &Nbt) -> Option<[BlockState; CELLS]> {
    let palette = match container.get("palette") {
        Some(Nbt::List(entries)) => entries,
        _ => return None,
    };
    let states: Vec<BlockState> = palette
        .iter()
        .map(|e| parse_block_palette_entry(e).map(BlockState::from))
        .collect::<Option<_>>()?;
    if states.is_empty() {
        return None;
    }

    let mut cells = [states::AIR; CELLS];
    if states.len() == 1 {
        // ZeroBitStorage: every cell is the sole palette entry.
        cells.fill(states[0]);
        return Some(cells);
    }

    let data = match container.get("data") {
        Some(Nbt::LongArray(longs)) => longs,
        _ => return None, // multi-entry palette must carry storage
    };
    let bits = block_storage_bits(states.len());
    let longs: Vec<u64> = data.iter().map(|&l| l as u64).collect();
    let indices = unpack_bits(&longs, bits, CELLS);
    for (cell, &index) in cells.iter_mut().zip(indices.iter()) {
        *cell = *states.get(index as usize)?;
    }
    Some(cells)
}

/// A single-value biome container carrying the chunk-centre biome name. Vela
/// re-derives per-column biomes from the generator on load (`from_nbt` ignores
/// the stored biomes), so storing one representative biome per section is
/// lossless for our pipeline while staying valid, readable NBT.
fn single_value_biomes(gen: &GenChunk) -> Nbt {
    Nbt::compound([(
        "palette",
        Nbt::List(vec![Nbt::string(gen.biome_name(8, 8))]),
    )])
}

/// One block-state palette entry: `{Name}` for a propertyless block, or
/// `{Name, Properties}` carrying every property's selected value.
fn block_palette_entry(state: u32) -> Nbt {
    match describe_state(state) {
        Some((name, props)) if props.is_empty() => Nbt::compound([("Name", Nbt::string(name))]),
        Some((name, props)) => Nbt::compound([
            ("Name", Nbt::string(name)),
            (
                "Properties",
                Nbt::compound(props.into_iter().map(|(k, v)| (k.to_string(), Nbt::string(v)))),
            ),
        ]),
        // An unknown id (should not occur for our own states) falls back to air
        // so the section still decodes rather than corrupting the palette.
        None => Nbt::compound([("Name", Nbt::string("minecraft:air"))]),
    }
}

/// Resolve a block-state palette entry `{Name, Properties?}` to a global id.
fn parse_block_palette_entry(entry: &Nbt) -> Option<u32> {
    let name = entry.get("Name").and_then(Nbt::as_str)?;
    let props: Vec<(&str, &str)> = match entry.get("Properties") {
        Some(Nbt::Compound(map)) => map
            .iter()
            .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
            .collect(),
        _ => Vec::new(),
    };
    with_properties(name, &props)
}

/// Disk storage width for a block-state palette of `len` entries, mirroring
/// `Strategy.createForBlockStates.getConfigurationForPaletteSize`: 0 bits for a
/// single value, otherwise `max(4, ceil(log2(len)))` (linear 4-bit floor, then
/// the exact bit count including the global-palette range).
fn block_storage_bits(len: usize) -> u32 {
    if len <= 1 {
        return 0;
    }
    let needed = usize::BITS - (len - 1).leading_zeros();
    needed.max(4)
}

/// The NBT key for a heightmap by its wire id (`Heightmap.Types.getSerializationKey`).
fn heightmap_key(id: i32) -> &'static str {
    match id {
        1 => "WORLD_SURFACE",
        4 => "MOTION_BLOCKING",
        _ => "WORLD_SURFACE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::storage::chunk_nbt;
    use crate::world::COLUMNS;

    /// Rebuild the dense grid straight from the baseline + edits, in the same
    /// section/cell order [`from_nbt`] returns — the expected round-trip value.
    fn expected_grid(gen: &GenChunk, edits: &HashMap<u32, BlockState>) -> Vec<BlockState> {
        let mut grid = vec![states::AIR; (SECTION_COUNT as usize) * CELLS];
        for section in 0..SECTION_COUNT {
            let base_y = MIN_Y + section * 16;
            for ly in 0..16i32 {
                let world_y = base_y + ly;
                for lz in 0..16i32 {
                    for lx in 0..16i32 {
                        let idx = (section as usize) * CELLS
                            + ((ly << 8) | (lz << 4) | lx) as usize;
                        grid[idx] = cell_state(gen, edits, lx, world_y, lz);
                    }
                }
            }
        }
        grid
    }

    #[test]
    fn block_storage_bits_matches_vanilla_widths() {
        assert_eq!(block_storage_bits(1), 0); // single value ⇒ no data
        assert_eq!(block_storage_bits(2), 4); // 4-bit linear floor
        assert_eq!(block_storage_bits(16), 4);
        assert_eq!(block_storage_bits(17), 5);
        assert_eq!(block_storage_bits(256), 8);
        assert_eq!(block_storage_bits(257), 9); // global-palette range
    }

    #[test]
    fn empty_chunk_round_trips() {
        // No edits: pure generated terrain must survive a to_nbt/from_nbt cycle.
        let gen = GenChunk::generate(3, -5);
        let edits = HashMap::new();
        let tag = chunk_nbt::to_nbt(3, -5, &gen, &edits, 42);
        let grid = chunk_nbt::from_nbt(&tag).expect("decode");
        assert_eq!(grid, expected_grid(&gen, &edits));
    }

    #[test]
    fn edited_chunk_round_trips_multi_state_sections() {
        // Place a handful of distinct blocks to force a multi-entry palette (and
        // thus a packed `data` array), then confirm the decode reproduces them.
        let gen = GenChunk::generate(0, 0);
        let mut edits = HashMap::new();
        // edit_key mirror: ((y-MIN_Y) * COLUMNS + lz*16 + lx).
        let key = |lx: i32, y: i32, lz: i32| {
            ((y - MIN_Y) as u32) * COLUMNS as u32 + (lz as u32) * 16 + lx as u32
        };
        edits.insert(key(1, 100, 1), BlockState(1)); // stone
        edits.insert(key(2, 100, 1), BlockState(10)); // dirt
        edits.insert(key(3, 100, 1), BlockState(85)); // bedrock
        edits.insert(key(1, 101, 1), BlockState(14)); // cobblestone

        let tag = chunk_nbt::to_nbt(0, 0, &gen, &edits, 0);
        let grid = chunk_nbt::from_nbt(&tag).expect("decode");
        assert_eq!(grid, expected_grid(&gen, &edits));

        // Spot-check one placed block sits where we put it.
        let section = ((100 - MIN_Y) / 16) as usize;
        let ly = (100 - MIN_Y) % 16;
        let idx = section * CELLS + ((ly << 8) | (1 << 4) | 1) as usize;
        assert_eq!(grid[idx], BlockState(1));
    }

    #[test]
    fn missing_status_is_rejected() {
        let tag = Nbt::compound([("sections", Nbt::List(vec![]))]);
        assert_eq!(chunk_nbt::from_nbt(&tag), None);
    }
}
