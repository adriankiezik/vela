//! Per-chunk wire encoding: the 24-section block blob, mirroring vanilla's
//! `LevelChunkSection` / `PalettedContainer` serialization.

use crate::protocol::buffer::PacketWriter;

use super::bitpack::pack_bits;
use super::chunk_data::cell_state;
use super::{states, CELLS, COLUMNS, MIN_Y, PLAINS_BIOME, SECTION_COUNT};

/// Encode the 24-section block blob for a chunk from its heights and edits.
pub(super) fn encode_blob(
    heights: &[i32; COLUMNS],
    edits: &std::collections::HashMap<u32, u32>,
) -> Vec<u8> {
    let mut out = PacketWriter::new();
    for section in 0..SECTION_COUNT {
        let base_y = MIN_Y + section * 16;
        encode_section(base_y, heights, edits, &mut out);
    }
    out.buf.to_vec()
}

/// Serialize one section: counts, then the block-state and biome containers.
/// `heights` are the 256 per-column surface heights for this chunk, indexed
/// `lz * 16 + lx`; `edits` overrides individual cells.
fn encode_section(
    base_y: i32,
    heights: &[i32; COLUMNS],
    edits: &std::collections::HashMap<u32, u32>,
    out: &mut PacketWriter,
) {
    // Cell index is vanilla's `(y << 8) | (z << 4) | x`.
    let mut cells = [states::AIR; CELLS];
    let mut non_air: u16 = 0;
    for ly in 0..16i32 {
        let world_y = base_y + ly;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let state = cell_state(heights, edits, lx, world_y, lz);
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
    write_single_value(PLAINS_BIOME, out); // biome container (single value)
}

/// Write a block-state `PalettedContainer`, mirroring vanilla
/// `PalettedContainer.Data.write` + `Strategy.createForBlockStates`:
///   - 1 distinct state: single-value palette (0 bits, no data array).
///   - 2..=256 distinct states: indirect palette (4-bit linear up to 16 entries,
///     then 5..=8-bit hashmap), written as a varint length + varint entries, then
///     the index data packed at that width.
///   - Over 256 distinct states: the direct/global palette — no palette list, the
///     data array carries raw global block-state ids. The client selects the
///     global palette whenever the bits byte exceeds 8
///     (`Strategy.getConfigurationForBitCount` `default` arm) and reads the data
///     at exactly that width, so we use the smallest width that both exceeds 8 and
///     is wide enough for the largest id present.
///
/// Generated terrain uses ≤5 states per section; only block placement can push a
/// section past the linear/hashmap ceiling, but the direct path is exercised for
/// correctness rather than silently corrupting the section at 9 bits.
fn write_block_palette(cells: &[u32; CELLS], out: &mut PacketWriter) {
    // First-seen distinct states. Sections are overwhelmingly uniform or hold a
    // handful of states, so a linear `Vec` scan beats a hashed map here (the map
    // allocation + hashing dominates for tiny palettes on this hot path).
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

    if palette.len() > 256 {
        // Direct/global palette: raw ids, no palette list.
        let max_state = cells.iter().copied().max().unwrap_or(0);
        let bits = bits_for_global(max_state);
        out.write_u8(bits as u8);
        let indices: Vec<u64> = cells.iter().map(|&c| c as u64).collect();
        for long in pack_bits(&indices, bits) {
            out.write_i64(long as i64);
        }
        return;
    }

    // Indirect palette: 4 bits minimum, widened to fit the entry count.
    let bits = bits_for_palette(palette.len());
    out.write_u8(bits as u8);
    out.write_varint(palette.len() as i32);
    for &state in &palette {
        out.write_varint(state as i32);
    }
    let indices: Vec<u64> = cells
        .iter()
        .map(|c| palette.iter().position(|p| p == c).unwrap() as u64)
        .collect();
    for long in pack_bits(&indices, bits) {
        out.write_i64(long as i64);
    }
}

/// Bits per entry for an indirect block-state palette of `len` entries: vanilla
/// pads to a 4-bit minimum, then uses the smallest width that indexes `len`.
fn bits_for_palette(len: usize) -> u32 {
    let needed = usize::BITS - (len - 1).leading_zeros();
    needed.max(4)
}

/// Bits per entry for a direct/global block-state palette holding raw ids up to
/// `max_state`: the smallest width that both exceeds the 8-bit indirect ceiling
/// (so the client decodes the section as global) and represents `max_state`.
fn bits_for_global(max_state: u32) -> u32 {
    let needed = u32::BITS - max_state.max(1).leading_zeros();
    needed.max(9)
}

/// A single-value (0-bit) `PalettedContainer`: just the value, no storage.
fn write_single_value(value: u32, out: &mut PacketWriter) {
    out.write_u8(0); // bits per entry
    out.write_varint(value as i32); // the sole palette entry
                                    // No data array follows a 0-bit storage.
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::chunk_data::chunk_heights;
    use super::super::terrain::state_at;

    #[test]
    fn bits_for_palette_widths() {
        assert_eq!(bits_for_palette(2), 4); // padded up to the 4-bit minimum
        assert_eq!(bits_for_palette(16), 4);
        assert_eq!(bits_for_palette(17), 5);
        assert_eq!(bits_for_palette(32), 5);
        assert_eq!(bits_for_palette(33), 6);
        assert_eq!(bits_for_palette(256), 8);
    }

    #[test]
    fn bits_for_global_is_at_least_9() {
        assert_eq!(bits_for_global(0), 9);
        assert_eq!(bits_for_global(300), 9);
        assert_eq!(bits_for_global(511), 9);
        assert_eq!(bits_for_global(512), 10);
    }

    #[test]
    fn over_256_distinct_states_uses_direct_palette() {
        // A section with >256 distinct states must serialize as the direct/global
        // palette: a >8 bits byte, NO palette list, then the raw-id data array.
        let mut cells = [states::AIR; CELLS];
        for (i, c) in cells.iter_mut().take(300).enumerate() {
            *c = i as u32 + 1; // ids 1..=300, all distinct
        }
        let mut out = PacketWriter::new();
        write_block_palette(&cells, &mut out);
        let bytes = out.buf.to_vec();

        assert!(bytes[0] > 8, "global palette requires a >8 bits byte");
        assert_eq!(bytes[0], 9); // max id 300 fits in 9 bits
        // No palette list follows: the rest is exactly the packed data array
        // (4096 cells at 9 bits, 7 per long).
        let per_long = 64 / 9;
        let longs = CELLS.div_ceil(per_long);
        assert_eq!(bytes.len(), 1 + longs * 8);
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
