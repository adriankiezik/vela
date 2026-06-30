//! The `SimpleBitStorage` packing primitive shared by the chunk-section block
//! encoding and the heightmaps.

/// Pack `values` into longs at `bits` each, vanilla `SimpleBitStorage` layout:
/// a value never straddles a long boundary, so each long holds `64 / bits`
/// values low-to-high and any leftover high bits stay zero.
pub(super) fn pack_bits(values: &[u64], bits: u32) -> Vec<u64> {
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
    use super::super::CELLS;
    use super::pack_bits;

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
}
