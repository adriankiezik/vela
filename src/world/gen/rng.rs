//! A deterministic 48-bit linear-congruential PRNG matching the well-documented
//! `java.util.Random` algorithm, plus vanilla's decoration-seed derivation.
//!
//! Clean-room: this is the *public* Java LCG (multiplier `0x5DEECE66D`, increment
//! `0xB`, 48-bit state) reimplemented from its published contract, not copied
//! source. Worldgen decorations in vanilla are seeded off the level seed and the
//! feature's origin block position via `WorldgenRandom.setDecorationSeed`, so
//! reproducing that seeding here gives per-chunk feature placement that is stable
//! across regeneration (which the persistence diff relies on).

/// LCG multiplier (`java.util.Random.multiplier`).
const MULT: u64 = 0x5DEE_CE66D;
/// LCG increment (`java.util.Random.addend`).
const ADD: u64 = 0xB;
/// 48-bit state mask (`java.util.Random.mask`).
const MASK: u64 = (1 << 48) - 1;

/// A `java.util.Random`-compatible generator over a 48-bit scrambled seed.
pub struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    /// Seed as `java.util.Random(long)` does: scramble with the multiplier.
    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ MULT) & MASK,
        }
    }

    /// Re-seed in place (`setSeed`).
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed as u64 ^ MULT) & MASK;
    }

    /// Advance the state and return the top `bits` bits (`next(int)`).
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(MULT).wrapping_add(ADD) & MASK;
        (self.seed >> (48 - bits)) as u32 as i32
    }

    /// A uniform int in `[0, bound)` (`nextInt(int)`), including the power-of-two
    /// fast path and the rejection loop for the general case.
    pub fn next_int(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        // Power of two: take the high bits of a 31-bit draw.
        if bound & bound.wrapping_neg() == bound {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            // Reject the biased tail (Java uses signed int overflow here).
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }

    /// A 64-bit draw (`nextLong`).
    pub fn next_long(&mut self) -> i64 {
        ((self.next(32) as i64) << 32).wrapping_add(self.next(32) as i64)
    }

    /// A float in `[0, 1)` (`nextFloat`).
    #[allow(dead_code)] // part of the RNG surface; not every draw type is used yet.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1u32 << 24) as f32
    }

    /// Seed this generator for a chunk's decoration pass from the level seed and
    /// the chunk-origin block position, mirroring
    /// `WorldgenRandom.setDecorationSeed(levelSeed, minBlockX, minBlockZ)`.
    pub fn set_decoration_seed(&mut self, level_seed: i64, block_x: i32, block_z: i32) {
        self.set_seed(level_seed);
        let a = self.next_long() | 1;
        let b = self.next_long() | 1;
        let seed = (block_x as i64)
            .wrapping_mul(a)
            .wrapping_add((block_z as i64).wrapping_mul(b))
            ^ level_seed;
        self.set_seed(seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_java_random_sequence() {
        // Reference values from java.util.Random(42): the first three next(32)
        // draws are stable and widely published.
        let mut r = JavaRandom::new(42);
        assert_eq!(r.next(32), -1170105035);
        assert_eq!(r.next(32), 234785527);
        assert_eq!(r.next(32), -1360544799);
    }

    #[test]
    fn next_int_bounded_is_in_range_and_deterministic() {
        let mut a = JavaRandom::new(123);
        let mut b = JavaRandom::new(123);
        for _ in 0..10_000 {
            let v = a.next_int(100);
            assert!((0..100).contains(&v));
            assert_eq!(v, b.next_int(100));
        }
    }

    #[test]
    fn decoration_seed_is_position_dependent() {
        let mut r = JavaRandom::new(0);
        r.set_decoration_seed(0x5EED, 0, 0);
        let s00 = r.next_int(1_000_000);
        r.set_decoration_seed(0x5EED, 16, 0);
        let s10 = r.next_int(1_000_000);
        assert_ne!(s00, s10, "adjacent chunks must decorate differently");
    }
}
