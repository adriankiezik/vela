//! Per-chunk wire encoding: the 24-section block blob, mirroring vanilla's
//! `LevelChunkSection` / `PalettedContainer` serialization.

use crate::ids::BlockState;
use crate::protocol::buffer::PacketWriter;

use super::bitpack::pack_bits;
use super::chunk_data::cell_state;
use super::gen::{biome_registry_size, GenChunk};
use super::{states, CELLS, MIN_Y, SECTION_COUNT};

/// Encode the 24-section block blob for a chunk from its generated baseline and
/// edits.
pub(super) fn encode_blob(
    gen: &GenChunk,
    edits: &std::collections::HashMap<u32, BlockState>,
) -> Vec<u8> {
    // Biomes are per-COLUMN in this data model: `write_biome_palette` reads only
    // `gen.biome_id(x, z)` (no vertical variation — see that function's doc), so
    // the encoded biome container is byte-identical for all 24 vertical sections.
    // Compute it once and splice the same bytes into each section rather than
    // rebuilding the palette 24× per chunk.
    let biome_bytes = {
        let mut bw = PacketWriter::new();
        write_biome_palette(gen, &mut bw);
        bw.buf.to_vec()
    };
    // Fast path for the overwhelmingly common prefetch case: a freshly generated
    // chunk has zero edits, so every cell is its baked baseline and we can skip
    // the per-cell `edit_key` + HashMap probe (98,304 probes/chunk) entirely.
    let edit_free = edits.is_empty();

    let mut out = PacketWriter::new();
    for section in 0..SECTION_COUNT {
        let base_y = MIN_Y + section * 16;
        encode_section(base_y, gen, edits, edit_free, &biome_bytes, &mut out);
    }
    out.buf.to_vec()
}

/// Serialize one section: counts, then the block-state and biome containers. The
/// baseline `gen` supplies the terrain/feature blocks and per-column biomes;
/// `edits` overrides individual cells. `edit_free` selects the no-edit fast path,
/// and `biome_bytes` is the once-per-chunk-precomputed biome container.
fn encode_section(
    base_y: i32,
    gen: &GenChunk,
    edits: &std::collections::HashMap<u32, BlockState>,
    edit_free: bool,
    biome_bytes: &[u8],
    out: &mut PacketWriter,
) {
    // Cell index is vanilla's `(y << 8) | (z << 4) | x`.
    let mut cells = [states::AIR; CELLS];
    let mut non_air: u16 = 0;
    for ly in 0..16i32 {
        let world_y = base_y + ly;
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                // Byte-identical to `cell_state`: with no edits present that
                // function always falls through to `gen.base_state`, so reading
                // the baseline directly here produces exactly the same state —
                // it just avoids the redundant edit-key hash and empty-map probe.
                let state = if edit_free {
                    gen.base_state(lx, world_y, lz)
                } else {
                    cell_state(gen, edits, lx, world_y, lz)
                };
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
    out.write_bytes(biome_bytes); // biome container (identical across sections)
}

/// Write the section's biome `PalettedContainer`. Biomes here are horizontal-only
/// (one per column), so a section samples 16 columns across its 4×4×4 grid. Uses
/// vanilla's biome `Strategy` (`createForBiomes`): single value, 1–3 bit linear
/// palette, else the global palette at `ceillog2(biomeCount)` bits.
fn write_biome_palette(gen: &GenChunk, out: &mut PacketWriter) {
    // 64 biome cells, indexed `(y << 2 | z) << 2 | x`; y is ignored (no vertical
    // biome variation), so each cell takes its column biome at local (x*4, z*4).
    let mut cells = [0u32; 64];
    for by in 0..4i32 {
        for bz in 0..4i32 {
            for bx in 0..4i32 {
                let idx = ((by << 2 | bz) << 2 | bx) as usize;
                cells[idx] = gen.biome_id(bx * 4, bz * 4);
            }
        }
    }

    // Distinct biome ids, first-seen (a handful at most).
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

    let bits = bits_for_palette_generic(palette.len());
    if bits <= 3 {
        // Linear indirect palette (biome strategy caps the indirect path at 3 bits).
        out.write_u8(bits as u8);
        out.write_varint(palette.len() as i32);
        for &id in &palette {
            out.write_varint(id as i32);
        }
        let indices: Vec<u64> = cells
            .iter()
            .map(|c| palette.iter().position(|p| p == c).unwrap() as u64)
            .collect();
        for long in pack_bits(&indices, bits) {
            out.write_i64(long as i64);
        }
        return;
    }

    // Global palette: raw biome ids at `ceillog2(biomeCount)` bits, no palette list.
    let global_bits = bits_for_global_count(biome_registry_size());
    out.write_u8(global_bits as u8);
    let indices: Vec<u64> = cells.iter().map(|&c| c as u64).collect();
    for long in pack_bits(&indices, global_bits) {
        out.write_i64(long as i64);
    }
}

/// Bits to index `len` distinct entries (`ceillog2`), with no minimum floor.
fn bits_for_palette_generic(len: usize) -> u32 {
    if len <= 1 {
        0
    } else {
        usize::BITS - (len - 1).leading_zeros()
    }
}

/// Bits for a global palette over `count` registry entries (`ceillog2(count)`).
fn bits_for_global_count(count: usize) -> u32 {
    if count <= 1 {
        1
    } else {
        usize::BITS - (count - 1).leading_zeros()
    }
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
fn write_block_palette(cells: &[BlockState; CELLS], out: &mut PacketWriter) {
    // First-seen distinct states. Sections are overwhelmingly uniform or hold a
    // handful of states, so a linear `Vec` scan beats a hashed map here (the map
    // allocation + hashing dominates for tiny palettes on this hot path).
    let mut palette: Vec<BlockState> = Vec::new();
    for &c in cells.iter() {
        if !palette.contains(&c) {
            palette.push(c);
        }
    }

    if palette.len() == 1 {
        write_single_value(palette[0].get(), out);
        return;
    }

    if palette.len() > 256 {
        // Direct/global palette: raw ids, no palette list. From here down we work
        // in raw palette integers — the values packed into the data array are bit
        // positions / ids, not `BlockState` identities.
        let max_state = cells.iter().map(|c| c.get()).max().unwrap_or(0);
        let bits = bits_for_global(max_state);
        out.write_u8(bits as u8);
        let indices: Vec<u64> = cells.iter().map(|&c| c.get() as u64).collect();
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
        out.write_varint(state.get() as i32);
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
            *c = BlockState(i as u32 + 1); // ids 1..=300, all distinct
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
    fn generated_section_palette_stays_indirect() {
        // Generation draws from a small block set, so every section should stay
        // well inside the 256-entry indirect-palette range (never forced global).
        let gen = GenChunk::generate(0, 0);
        for section in 0..SECTION_COUNT {
            let base_y = MIN_Y + section * 16;
            let mut distinct: Vec<BlockState> = Vec::new();
            for ly in 0..16i32 {
                for lz in 0..16i32 {
                    for lx in 0..16i32 {
                        let s = gen.base_state(lx, base_y + ly, lz);
                        if !distinct.contains(&s) {
                            distinct.push(s);
                        }
                    }
                }
            }
            assert!(
                distinct.len() <= 256,
                "section {section} has {} states (would force the global palette)",
                distinct.len()
            );
        }
    }

    #[test]
    fn biome_container_is_written_per_section() {
        // The block blob must be non-trivial and encode a biome container after
        // each section's block container (single-value or small linear palette).
        let gen = GenChunk::generate(0, 0);
        let blob = encode_blob(&gen, &std::collections::HashMap::new());
        assert!(blob.len() > (SECTION_COUNT as usize) * 8);
    }
}
