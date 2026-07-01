//! Vanilla-parity worldgen randomness: `Xoroshiro128++`, the 128-bit seed
//! derivations, positional forking, and `WorldgenRandom`'s seed setters.
//!
//! This is the P0 layer of the 1:1 worldgen port (docs/WORLDGEN_PARITY.md):
//! every noise instance, carver, and feature draw in vanilla flows from the
//! world seed through these exact mixing steps, so all of them must match
//! bit-for-bit. Mirrors `XoroshiroRandomSource`, `LegacyRandomSource`,
//! `RandomSupport`, `WorldgenRandom`, and `Mth.getSeed`. Golden values in the
//! tests were captured from a JVM harness running the reference arithmetic.
//!
//! Clean-room note: xoroshiro128++ is Blackman & Vigna's public-domain
//! generator, Stafford's mix13 and the Java LCG are published algorithms, and
//! the seed plumbing constants are observable data.

use md5::{Digest, Md5};

/// `BitRandomSource.FLOAT_MULTIPLIER` / `XoroshiroRandomSource.FLOAT_UNIT`.
const FLOAT_UNIT: f32 = 5.960_464_5E-8;
/// Vanilla declares this `double` from a *float* literal (`1.110223E-16F`);
/// that float rounds to exactly `2^-53`, so the widened value equals
/// `java.util.Random`'s DOUBLE_UNIT.
const DOUBLE_UNIT: f64 = 1.110_223E-16_f32 as f64;

const GOLDEN_RATIO_64: i64 = -7046029254386353131;
const SILVER_RATIO_64: i64 = 7640891576956012809;

// ---------------------------------------------------------------------------
// Seed support (`RandomSupport`)
// ---------------------------------------------------------------------------

/// Stafford variant 13 of the SplitMix64 finalizer (`mixStafford13`).
pub fn mix_stafford_13(mut z: i64) -> i64 {
    z = (z ^ ((z as u64) >> 30) as i64).wrapping_mul(-4658895280553007687);
    z = (z ^ ((z as u64) >> 27) as i64).wrapping_mul(-7723592293110705685);
    z ^ ((z as u64) >> 31) as i64
}

/// A 128-bit xoroshiro seed (`RandomSupport.Seed128bit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seed128 {
    pub lo: i64,
    pub hi: i64,
}

impl Seed128 {
    pub fn xor(self, lo: i64, hi: i64) -> Seed128 {
        Seed128 {
            lo: self.lo ^ lo,
            hi: self.hi ^ hi,
        }
    }

    pub fn mixed(self) -> Seed128 {
        Seed128 {
            lo: mix_stafford_13(self.lo),
            hi: mix_stafford_13(self.hi),
        }
    }
}

/// `RandomSupport.upgradeSeedTo128bit`: golden/silver-ratio spread + mix13.
pub fn upgrade_seed_to_128bit(seed: i64) -> Seed128 {
    let lo = seed ^ SILVER_RATIO_64;
    let hi = lo.wrapping_add(GOLDEN_RATIO_64);
    Seed128 { lo, hi }.mixed()
}

/// `RandomSupport.seedFromHashOf`: the two big-endian halves of `md5(input)`.
pub fn seed_from_hash_of(input: &str) -> Seed128 {
    let digest = Md5::digest(input.as_bytes());
    let lo = i64::from_be_bytes(digest[0..8].try_into().unwrap());
    let hi = i64::from_be_bytes(digest[8..16].try_into().unwrap());
    Seed128 { lo, hi }
}

/// `Mth.getSeed(x, y, z)`: the block-position hash behind positional `at`.
pub fn mth_get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut seed =
        (x.wrapping_mul(3129871) as i64) ^ (z as i64).wrapping_mul(116129781) ^ y as i64;
    seed = seed
        .wrapping_mul(seed)
        .wrapping_mul(42317861)
        .wrapping_add(seed.wrapping_mul(11));
    seed >> 16
}

/// `String.hashCode()` over UTF-16 code units, for the legacy `fromHashOf`.
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

// ---------------------------------------------------------------------------
// Xoroshiro128++ core
// ---------------------------------------------------------------------------

/// The raw xoroshiro128++ engine (`Xoroshiro128PlusPlus`).
#[derive(Clone, Debug)]
pub struct Xoroshiro128PlusPlus {
    lo: i64,
    hi: i64,
}

impl Xoroshiro128PlusPlus {
    pub fn new(lo: i64, hi: i64) -> Self {
        if lo | hi == 0 {
            // All-zero state is invalid; vanilla substitutes the ratio pair.
            Self {
                lo: GOLDEN_RATIO_64,
                hi: SILVER_RATIO_64,
            }
        } else {
            Self { lo, hi }
        }
    }

    pub fn from_seed(seed: Seed128) -> Self {
        Self::new(seed.lo, seed.hi)
    }

    pub fn next_long(&mut self) -> i64 {
        let s0 = self.lo;
        let mut s1 = self.hi;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.hi = s1.rotate_left(28);
        result
    }
}

// ---------------------------------------------------------------------------
// Random sources
// ---------------------------------------------------------------------------

/// A seedable vanilla random source: either algorithm behind one surface
/// (`RandomSource` + `WorldgenRandom.Algorithm`).
#[derive(Clone, Debug)]
pub enum RandomSource {
    Legacy(LegacyRandom),
    Xoroshiro(XoroshiroRandom),
}

impl RandomSource {
    pub fn legacy(seed: i64) -> Self {
        RandomSource::Legacy(LegacyRandom::new(seed))
    }

    pub fn xoroshiro(seed: i64) -> Self {
        RandomSource::Xoroshiro(XoroshiroRandom::new(seed))
    }

    pub fn fork(&mut self) -> RandomSource {
        match self {
            RandomSource::Legacy(r) => RandomSource::Legacy(LegacyRandom::new(r.next_long())),
            RandomSource::Xoroshiro(r) => {
                let lo = r.rng.next_long();
                let hi = r.rng.next_long();
                RandomSource::Xoroshiro(XoroshiroRandom::from_seed128(Seed128 { lo, hi }))
            }
        }
    }

    pub fn fork_positional(&mut self) -> PositionalRandomFactory {
        match self {
            RandomSource::Legacy(r) => PositionalRandomFactory::Legacy { seed: r.next_long() },
            RandomSource::Xoroshiro(r) => PositionalRandomFactory::Xoroshiro {
                lo: r.rng.next_long(),
                hi: r.rng.next_long(),
            },
        }
    }

    pub fn set_seed(&mut self, seed: i64) {
        match self {
            RandomSource::Legacy(r) => r.set_seed(seed),
            RandomSource::Xoroshiro(r) => r.set_seed(seed),
        }
    }

    pub fn next_int(&mut self) -> i32 {
        match self {
            RandomSource::Legacy(r) => r.next(32),
            RandomSource::Xoroshiro(r) => r.rng.next_long() as i32,
        }
    }

    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        match self {
            RandomSource::Legacy(r) => r.next_int_bounded(bound),
            RandomSource::Xoroshiro(r) => r.next_int_bounded(bound),
        }
    }

    /// `nextInt(origin, bound)` — uniform in `[origin, bound)`.
    pub fn next_int_between(&mut self, origin: i32, bound: i32) -> i32 {
        debug_assert!(origin < bound);
        origin + self.next_int_bounded(bound - origin)
    }

    /// `nextIntBetweenInclusive(min, max)`.
    pub fn next_int_inclusive(&mut self, min: i32, max: i32) -> i32 {
        self.next_int_bounded(max - min + 1) + min
    }

    pub fn next_long(&mut self) -> i64 {
        match self {
            RandomSource::Legacy(r) => r.next_long(),
            RandomSource::Xoroshiro(r) => r.rng.next_long(),
        }
    }

    pub fn next_boolean(&mut self) -> bool {
        match self {
            RandomSource::Legacy(r) => r.next(1) != 0,
            RandomSource::Xoroshiro(r) => r.rng.next_long() & 1 != 0,
        }
    }

    pub fn next_float(&mut self) -> f32 {
        match self {
            RandomSource::Legacy(r) => r.next(24) as f32 * FLOAT_UNIT,
            RandomSource::Xoroshiro(r) => r.next_bits(24) as f32 * FLOAT_UNIT,
        }
    }

    pub fn next_double(&mut self) -> f64 {
        match self {
            RandomSource::Legacy(r) => r.next_double(),
            RandomSource::Xoroshiro(r) => r.next_bits(53) as f64 * DOUBLE_UNIT,
        }
    }

    pub fn next_gaussian(&mut self) -> f64 {
        // MarsagliaPolarGaussian: the cached second sample lives per source.
        let mut cached = match self {
            RandomSource::Legacy(r) => r.gaussian.take(),
            RandomSource::Xoroshiro(r) => r.gaussian.take(),
        };
        if let Some(g) = cached.take() {
            return g;
        }
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let r2 = x * x + y * y;
            if r2 < 1.0 && r2 != 0.0 {
                let m = (-2.0 * r2.ln() / r2).sqrt();
                let second = y * m;
                match self {
                    RandomSource::Legacy(r) => r.gaussian = Some(second),
                    RandomSource::Xoroshiro(r) => r.gaussian = Some(second),
                }
                return x * m;
            }
        }
    }

    /// `triangle(mean, spread)` — used all over feature placement.
    pub fn triangle(&mut self, mean: f64, spread: f64) -> f64 {
        mean + spread * (self.next_double() - self.next_double())
    }

    /// `consumeCount`: legacy burns `nextInt()` per round, xoroshiro overrides
    /// to burn a full `nextLong()` per round.
    pub fn consume_count(&mut self, rounds: u32) {
        for _ in 0..rounds {
            match self {
                RandomSource::Legacy(r) => {
                    r.next(32);
                }
                RandomSource::Xoroshiro(r) => {
                    r.rng.next_long();
                }
            }
        }
    }
}

/// `LegacyRandomSource`: the 48-bit Java LCG (single-threaded — vanilla's
/// atomics only guard against misuse).
#[derive(Clone, Debug)]
pub struct LegacyRandom {
    seed: i64,
    gaussian: Option<f64>,
}

const LCG_MULT: i64 = 25214903917;
const LCG_ADD: i64 = 11;
const LCG_MASK: i64 = (1 << 48) - 1;

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ LCG_MULT) & LCG_MASK,
            gaussian: None,
        }
    }

    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ LCG_MULT) & LCG_MASK;
        self.gaussian = None;
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(LCG_MULT).wrapping_add(LCG_ADD) & LCG_MASK;
        (self.seed >> (48 - bits)) as i32
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        if bound & bound.wrapping_sub(1) == 0 {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    fn next_long(&mut self) -> i64 {
        ((self.next(32) as i64) << 32).wrapping_add(self.next(32) as i64)
    }

    fn next_double(&mut self) -> f64 {
        let upper = self.next(26);
        let lower = self.next(27);
        (((upper as i64) << 27) + lower as i64) as f64 * DOUBLE_UNIT
    }
}

/// `XoroshiroRandomSource`.
#[derive(Clone, Debug)]
pub struct XoroshiroRandom {
    rng: Xoroshiro128PlusPlus,
    gaussian: Option<f64>,
}

impl XoroshiroRandom {
    pub fn new(seed: i64) -> Self {
        Self::from_seed128(upgrade_seed_to_128bit(seed))
    }

    pub fn from_seed128(seed: Seed128) -> Self {
        Self {
            rng: Xoroshiro128PlusPlus::from_seed(seed),
            gaussian: None,
        }
    }

    pub fn set_seed(&mut self, seed: i64) {
        self.rng = Xoroshiro128PlusPlus::from_seed(upgrade_seed_to_128bit(seed));
        self.gaussian = None;
    }

    fn next_bits(&mut self, bits: u32) -> i64 {
        ((self.rng.next_long() as u64) >> (64 - bits)) as i64
    }

    /// Lemire-style bounded draw with rejection (`nextInt(int)`).
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        let mut bits = (self.rng.next_long() as i32) as u32 as u64;
        let mut product = bits * bound as u64;
        let mut fractional = product & 0xFFFF_FFFF;
        if fractional < bound as u64 {
            let threshold = (bound.wrapping_neg() as u32 % bound as u32) as u64;
            while fractional < threshold {
                bits = (self.rng.next_long() as i32) as u32 as u64;
                product = bits * bound as u64;
                fractional = product & 0xFFFF_FFFF;
            }
        }
        (product >> 32) as i32
    }
}

// ---------------------------------------------------------------------------
// Positional forking
// ---------------------------------------------------------------------------

/// `PositionalRandomFactory` for both algorithms: derives child sources from
/// positions, resource-location names, or plain seeds.
#[derive(Clone, Copy, Debug)]
pub enum PositionalRandomFactory {
    Legacy { seed: i64 },
    Xoroshiro { lo: i64, hi: i64 },
}

impl PositionalRandomFactory {
    pub fn at(&self, x: i32, y: i32, z: i32) -> RandomSource {
        let pos = mth_get_seed(x, y, z);
        match *self {
            PositionalRandomFactory::Legacy { seed } => RandomSource::legacy(pos ^ seed),
            // Only the low word takes the positional hash; the high word is
            // the factory's, unchanged.
            PositionalRandomFactory::Xoroshiro { lo, hi } => RandomSource::Xoroshiro(
                XoroshiroRandom::from_seed128(Seed128 { lo: pos ^ lo, hi }),
            ),
        }
    }

    pub fn from_hash_of(&self, name: &str) -> RandomSource {
        match *self {
            PositionalRandomFactory::Legacy { seed } => {
                RandomSource::legacy(java_string_hash(name) as i64 ^ seed)
            }
            PositionalRandomFactory::Xoroshiro { lo, hi } => RandomSource::Xoroshiro(
                XoroshiroRandom::from_seed128(seed_from_hash_of(name).xor(lo, hi)),
            ),
        }
    }

    pub fn from_seed(&self, seed: i64) -> RandomSource {
        match *self {
            PositionalRandomFactory::Legacy { .. } => RandomSource::legacy(seed),
            // Deliberately un-mixed in vanilla: the raw xor pair is the state.
            PositionalRandomFactory::Xoroshiro { lo, hi } => {
                RandomSource::Xoroshiro(XoroshiroRandom::from_seed128(Seed128 {
                    lo: seed ^ lo,
                    hi: seed ^ hi,
                }))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WorldgenRandom
// ---------------------------------------------------------------------------

/// `WorldgenRandom`: wraps a source but derives *all* draw types from
/// `next(bits)` (the `BitRandomSource` defaults) — so over a xoroshiro inner,
/// `nextLong()` consumes TWO engine longs (top 32 bits of each). That quirk is
/// parity-relevant for every carver and decoration draw.
#[derive(Clone, Debug)]
pub struct WorldgenRandom {
    inner: RandomSource,
    count: u32,
    gaussian: Option<f64>,
}

impl WorldgenRandom {
    pub fn new(inner: RandomSource) -> Self {
        Self {
            inner,
            count: 0,
            gaussian: None,
        }
    }

    /// Draws consumed so far (`getCount`, used by vanilla debug).
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn fork(&mut self) -> RandomSource {
        self.inner.fork()
    }

    pub fn fork_positional(&mut self) -> PositionalRandomFactory {
        self.inner.fork_positional()
    }

    /// Note: vanilla does *not* reset the gaussian cache here (unlike the
    /// sources' own `setSeed`).
    pub fn set_seed(&mut self, seed: i64) {
        self.inner.set_seed(seed);
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.count += 1;
        match &mut self.inner {
            RandomSource::Legacy(r) => r.next(bits),
            RandomSource::Xoroshiro(r) => ((r.rng.next_long() as u64) >> (64 - bits)) as i32,
        }
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        if bound & bound.wrapping_sub(1) == 0 {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    pub fn next_int_between(&mut self, origin: i32, bound: i32) -> i32 {
        debug_assert!(origin < bound);
        origin + self.next_int_bounded(bound - origin)
    }

    pub fn next_long(&mut self) -> i64 {
        ((self.next(32) as i64) << 32).wrapping_add(self.next(32) as i64)
    }

    pub fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * FLOAT_UNIT
    }

    pub fn next_double(&mut self) -> f64 {
        let upper = self.next(26);
        let lower = self.next(27);
        (((upper as i64) << 27) + lower as i64) as f64 * DOUBLE_UNIT
    }

    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(g) = self.gaussian.take() {
            return g;
        }
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let r2 = x * x + y * y;
            if r2 < 1.0 && r2 != 0.0 {
                let m = (-2.0 * r2.ln() / r2).sqrt();
                self.gaussian = Some(y * m);
                return x * m;
            }
        }
    }

    /// `setDecorationSeed(levelSeed, minBlockX, minBlockZ)` — returns the
    /// decoration seed that `setFeatureSeed` later offsets.
    pub fn set_decoration_seed(&mut self, level_seed: i64, min_block_x: i32, min_block_z: i32) -> i64 {
        self.set_seed(level_seed);
        let x_scale = self.next_long() | 1;
        let z_scale = self.next_long() | 1;
        let seed = (min_block_x as i64)
            .wrapping_mul(x_scale)
            .wrapping_add((min_block_z as i64).wrapping_mul(z_scale))
            ^ level_seed;
        self.set_seed(seed);
        seed
    }

    /// `setFeatureSeed(decorationSeed, featureIndex, generationStep)`.
    pub fn set_feature_seed(&mut self, decoration_seed: i64, index: i32, step: i32) {
        let seed = decoration_seed
            .wrapping_add(index as i64)
            .wrapping_add(10_000i64.wrapping_mul(step as i64));
        self.set_seed(seed);
    }

    /// `setLargeFeatureSeed(baseSeed, chunkX, chunkZ)` — carver seeding.
    pub fn set_large_feature_seed(&mut self, base_seed: i64, chunk_x: i32, chunk_z: i32) {
        self.set_seed(base_seed);
        let x_scale = self.next_long();
        let z_scale = self.next_long();
        let seed = (chunk_x as i64).wrapping_mul(x_scale) ^ (chunk_z as i64).wrapping_mul(z_scale) ^ base_seed;
        self.set_seed(seed);
    }

    /// `setLargeFeatureWithSalt(levelSeed, regionX, regionZ, salt)` — structure
    /// placement seeding.
    pub fn set_large_feature_with_salt(&mut self, level_seed: i64, x: i32, z: i32, salt: i32) {
        let seed = (x as i64)
            .wrapping_mul(341873128712)
            .wrapping_add((z as i64).wrapping_mul(132897987541))
            .wrapping_add(level_seed)
            .wrapping_add(salt as i64);
        self.set_seed(seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All golden values below were produced by a JVM harness running the
    // reference arithmetic (Java 25).

    #[test]
    fn xoroshiro_raw_sequence() {
        let mut r = Xoroshiro128PlusPlus::new(1, 2);
        assert_eq!(r.next_long(), 393217);
        assert_eq!(r.next_long(), 669327710093319);
        assert_eq!(r.next_long(), 1732421326133921491);
        assert_eq!(r.next_long(), -7051953992050424633);
        assert_eq!(r.next_long(), -8891291296936358940);
    }

    #[test]
    fn xoroshiro_zero_seed_fallback() {
        let mut r = Xoroshiro128PlusPlus::new(0, 0);
        assert_eq!(r.next_long(), 6807859099481836695);
    }

    #[test]
    fn stafford_mix() {
        assert_eq!(mix_stafford_13(0), 0);
        assert_eq!(mix_stafford_13(1), 6238072747940578789);
        assert_eq!(mix_stafford_13(-1), -5417735806833148549);
        assert_eq!(mix_stafford_13(123456789), -1000775671988880032);
    }

    #[test]
    fn seed_upgrade() {
        assert_eq!(
            upgrade_seed_to_128bit(42),
            Seed128 { lo: 6720814022939733433, hi: -2851323883594622011 }
        );
        assert_eq!(
            upgrade_seed_to_128bit(0),
            Seed128 { lo: 3847398142028685078, hi: 7192185014346937746 }
        );
        assert_eq!(
            upgrade_seed_to_128bit(-4972807208247972243),
            Seed128 { lo: 1052890561124616066, hi: -7362803455296458427 }
        );
    }

    #[test]
    fn md5_seed_from_hash() {
        assert_eq!(
            seed_from_hash_of("minecraft:temperature"),
            Seed128 { lo: 6664882324328353151, hi: -587597586455377528 }
        );
        assert_eq!(
            seed_from_hash_of("octave_-7"),
            Seed128 { lo: -1075682932162398897, hi: 2700503254851170474 }
        );
    }

    #[test]
    fn mth_get_seed_matches_java() {
        assert_eq!(mth_get_seed(12345, -678, 9012), -70485982246135);
        assert_eq!(mth_get_seed(0, 0, 0), 0);
        assert_eq!(mth_get_seed(-30000000, 64, 30000000), -35216720112214);
    }

    #[test]
    fn java_string_hash_matches() {
        assert_eq!(java_string_hash("minecraft:temperature"), -549971161);
        assert_eq!(java_string_hash(""), 0);
    }

    #[test]
    fn xoroshiro_source_draws() {
        let mut r = RandomSource::xoroshiro(42);
        assert_eq!(r.next_long(), -4695948378737616609);
        assert_eq!(r.next_long(), 7341713790291473579);
        assert_eq!(r.next_int(), -610653507);
        assert_eq!(r.next_int_bounded(17), 8);
        assert_eq!(r.next_int_bounded(17), 11);
        assert_eq!(r.next_float(), 0.64807147);
        assert_eq!(r.next_double(), 0.44883000173893806);
        assert!(!r.next_boolean());
    }

    #[test]
    fn xoroshiro_positional_factory() {
        let mut r = RandomSource::xoroshiro(42);
        let factory = r.fork_positional();
        assert_eq!(factory.at(12, -34, 56).next_long(), 7277768864249331706);
        assert_eq!(
            factory.from_hash_of("minecraft:temperature").next_long(),
            5928662810780352044
        );
        assert_eq!(factory.from_seed(999).next_long(), 726233142692553831);
    }

    #[test]
    fn legacy_source_draws() {
        let mut r = RandomSource::legacy(42);
        assert_eq!(r.next_long(), -5025562857975149833);
        assert_eq!(r.next_double(), 0.6832234717598454);

        let mut r = RandomSource::legacy(42);
        let factory = r.fork_positional();
        assert_eq!(factory.at(12, -34, 56).next_long(), 1065526125927453149);
        assert_eq!(
            factory.from_hash_of("minecraft:temperature").next_long(),
            8701972507863024934
        );
    }

    #[test]
    fn worldgen_random_over_xoroshiro() {
        let world_seed: i64 = 8677741122156433366;
        let mut r = WorldgenRandom::new(RandomSource::xoroshiro(0));

        let deco = r.set_decoration_seed(world_seed, -48, 96);
        assert_eq!(deco, 1490910010415781158);
        assert_eq!(r.next_long(), -9010806237127285168);
        assert_eq!(r.next_int_bounded(1000), 898);

        r.set_feature_seed(deco, 3, 7);
        assert_eq!(r.next_int_bounded(16), 4);

        r.set_large_feature_seed(world_seed, -3, 14);
        assert_eq!(r.next_int_bounded(24), 16);

        r.set_large_feature_with_salt(world_seed, -3, 14, 30005);
        assert_eq!(r.next_int_bounded(24), 12);
    }

    #[test]
    fn worldgen_random_over_legacy_matches_source() {
        // Over a legacy inner, WorldgenRandom must be draw-for-draw identical
        // to the bare source (both route through the same next(bits)).
        let mut wg = WorldgenRandom::new(RandomSource::legacy(0));
        wg.set_seed(1234);
        let mut src = RandomSource::legacy(1234);
        for _ in 0..64 {
            assert_eq!(wg.next_int_bounded(100), src.next_int_bounded(100));
        }
        assert_eq!(wg.next_long(), src.next_long());
        assert_eq!(wg.next_double(), src.next_double());
    }

    #[test]
    fn fork_and_consume() {
        // fork() must draw exactly two engine longs on xoroshiro.
        let mut a = RandomSource::xoroshiro(7);
        let mut b = RandomSource::xoroshiro(7);
        let _child = a.fork();
        b.next_long();
        b.next_long();
        assert_eq!(a.next_long(), b.next_long());

        // consumeCount on xoroshiro burns one long per round.
        let mut a = RandomSource::xoroshiro(7);
        let mut b = RandomSource::xoroshiro(7);
        a.consume_count(3);
        for _ in 0..3 {
            b.next_long();
        }
        assert_eq!(a.next_long(), b.next_long());
    }
}
