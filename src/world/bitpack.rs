//! The `SimpleBitStorage` packing primitive shared by the chunk-section block
//! encoding and the heightmaps.

/// Pack `values` into longs at `bits` each, vanilla `SimpleBitStorage` layout:
/// a value never straddles a long boundary, so each long holds `64 / bits`
/// values low-to-high and any leftover high bits stay zero.
pub(super) fn pack_bits(values: &[u64], bits: u32) -> Vec<u64> {
    let per_long = (64 / bits) as usize;
    let long_count = values.len().div_ceil(per_long);
    // Mask each value to `bits` before OR-ing it in, matching vanilla
    // `SimpleBitStorage` (`values[...] & this.mask`) so out-of-range bits can
    // never bleed into the neighbouring slot. Defensive: current callers are all
    // in range.
    let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    let mut longs = vec![0u64; long_count];
    for (i, &v) in values.iter().enumerate() {
        let long = i / per_long;
        let offset = (i % per_long) as u32 * bits;
        longs[long] |= (v & mask) << offset;
    }
    longs
}

/// Unpack the first `count` values (`bits` each) from a `SimpleBitStorage` long
/// array — the inverse of [`pack_bits`]. Values are read low-to-high within each
/// long and never straddle a long boundary. `longs` need only cover the values
/// asked for (`ceil(count / (64 / bits))` entries); a missing trailing long reads
/// as zero. Shared by the chunk-section storage read path.
pub(super) fn unpack_bits(longs: &[u64], bits: u32, count: usize) -> Vec<u64> {
    let per_long = (64 / bits) as usize;
    let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let long = longs.get(i / per_long).copied().unwrap_or(0);
        let offset = (i % per_long) as u32 * bits;
        out.push((long >> offset) & mask);
    }
    out
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

    #[test]
    fn unpack_inverts_pack() {
        use super::unpack_bits;
        // Round-trip a spread of widths and values through pack/unpack.
        for bits in [1u32, 4, 5, 9, 15] {
            let max = (1u64 << bits) - 1;
            let values: Vec<u64> = (0..1000u64).map(|i| (i.wrapping_mul(2654435761)) & max).collect();
            let longs = pack_bits(&values, bits);
            let back = unpack_bits(&longs, bits, values.len());
            assert_eq!(back, values, "bits={bits}");
        }
    }

    #[test]
    fn unpack_reads_low_to_high() {
        use super::unpack_bits;
        // The 0x321 long holds 4-bit values 1,2,3 in ascending nibbles.
        assert_eq!(unpack_bits(&[0x321], 4, 3), vec![1, 2, 3]);
    }
}
