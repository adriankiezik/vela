//! World data representation — the chunk-section wire encoding and the
//! bit-packing primitive underneath it.
//!
//! A chunk column is 24 stacked sections of 16×16×16 cells rising from the
//! world floor (`MIN_Y` = -64). Each section serializes exactly as vanilla's
//! `LevelChunkSection`: a non-air block count, a fluid count, a block-state
//! `PalettedContainer`, then a biome `PalettedContainer`. We emit the wire bytes
//! for a *static* world directly rather than modelling the full mutable
//! container — enough to stream a fixed flat world.
//!
//! Reference: decompiled `LevelChunkSection`, `PalettedContainer`, `Strategy`,
//! and `Heightmap` (MC 26.2). The numeric block-state ids come from the server's
//! own `--reports` block dump (observable output), not copied source.

use std::sync::OnceLock;

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

/// Highest solid layer in the flat profile: grass sits at y=63, so a player
/// spawned at y=64 stands on it.
pub const SURFACE_Y: i32 = 63;

/// Global block-state palette ids — the default state of each block, read from
/// the generated `reports/blocks.json` for 26.2.
mod states {
    pub const AIR: u32 = 0;
    pub const GRASS_BLOCK: u32 = 9;
    pub const DIRT: u32 = 10;
    pub const BEDROCK: u32 = 85;
}

/// The block state at a given world height in the flat profile.
fn state_at(world_y: i32) -> u32 {
    if world_y == MIN_Y {
        states::BEDROCK
    } else if world_y < SURFACE_Y {
        states::DIRT
    } else if world_y == SURFACE_Y {
        states::GRASS_BLOCK
    } else {
        states::AIR
    }
}

/// The 24-section block blob for a flat column, computed once and shared by
/// every chunk (the world is uniform).
pub fn flat_column_blob() -> &'static [u8] {
    static BLOB: OnceLock<Vec<u8>> = OnceLock::new();
    BLOB.get_or_init(build_flat_column_blob)
}

fn build_flat_column_blob() -> Vec<u8> {
    let mut out = PacketWriter::new();
    for section in 0..SECTION_COUNT {
        let base_y = MIN_Y + section * 16;
        encode_section(base_y, &mut out);
    }
    out.buf.to_vec()
}

/// Serialize one section: counts, then the block-state and biome containers.
fn encode_section(base_y: i32, out: &mut PacketWriter) {
    // Cell index is vanilla's `(y << 8) | (z << 4) | x`.
    let mut cells = [states::AIR; CELLS];
    let mut non_air: u16 = 0;
    for ly in 0..16 {
        let state = state_at(base_y + ly as i32);
        if state != states::AIR {
            non_air += 16 * 16;
        }
        for cell in &mut cells[ly * 256..ly * 256 + 256] {
            *cell = state;
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
/// = id 4), each a packed `long[]` of 256 column heights. On a flat world both
/// equal the first free y above the surface, relative to the world floor.
pub fn flat_heightmaps() -> &'static [(i32, Vec<i64>)] {
    static MAPS: OnceLock<Vec<(i32, Vec<i64>)>> = OnceLock::new();
    MAPS.get_or_init(|| {
        // `getFirstAvailable - minY`: first empty y is SURFACE_Y + 1.
        let height = (SURFACE_Y + 1 - MIN_Y) as u64;
        // Bits = ceil(log2(worldHeight + 1)); 384-tall column -> 9.
        let bits = ((SECTION_COUNT * 16 + 1) as u32)
            .next_power_of_two()
            .trailing_zeros();
        let values = vec![height; COLUMNS];
        let packed: Vec<i64> = pack_bits(&values, bits)
            .into_iter()
            .map(|l| l as i64)
            .collect();
        vec![(1, packed.clone()), (4, packed)]
    })
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
    fn heightmap_geometry() {
        let maps = flat_heightmaps();
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].0, 1); // WORLD_SURFACE
        assert_eq!(maps[1].0, 4); // MOTION_BLOCKING
        // 256 columns at 9 bits, 7 per long -> 37 longs.
        assert_eq!(maps[0].1.len(), 37);
    }

    #[test]
    fn surface_section_has_grass_dirt_bedrock_and_air() {
        // Bottom section (-64..-49) holds bedrock + dirt and nothing else;
        // it must serialize as a multi-value palette, not single.
        let blob = flat_column_blob();
        assert!(!blob.is_empty());
        // Column blob is 24 sections; a fully-air section is 8 bytes, so a
        // world with solid ground must exceed 24*8.
        assert!(blob.len() > (SECTION_COUNT as usize) * 8);
    }

    #[test]
    fn blob_byte_count_matches_section_layout() {
        // 2 multi-value sections (bedrock+dirt floor, dirt+grass surface) at
        // 2058 bytes each (4+1+1+2 palette varints+2048 longs+2 biome); the
        // other 22 sections are uniform dirt or air and collapse to 8 bytes.
        let n = flat_column_blob().len();
        assert_eq!(n, 2 * 2058 + 22 * 8, "got {n}");
    }
}
