//! Vanilla-parity noise primitives (`levelgen/synth`): `ImprovedNoise`,
//! `PerlinNoise`, `NormalNoise`, `BlendedNoise`.
//!
//! P1 of the 1:1 worldgen port (docs/WORLDGEN_PARITY.md). Every value here is
//! bit-for-bit against the reference: permutation tables consume the RNG in
//! construction order, octaves seed via `fromHashOf("octave_N")` (new mode) or
//! sequential draws with 262-step skips (legacy mode), and the smear/fudge
//! variant of `noise()` keeps its float-epsilon quirk. Golden values in the
//! tests were captured from a JVM harness running the reference arithmetic.

// Consumed by the P2 density-function engine; until that lands only the tests
// exercise this module.
#![allow(dead_code)]

use super::random::RandomSource;

/// The classic Perlin gradient table (`SimplexNoise.GRADIENT`), 16 entries
/// with the 4 duplicated padding rows.
const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

fn dot(g: &[i32; 3], x: f64, y: f64, z: f64) -> f64 {
    g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z
}

/// `Mth.lerp`.
pub fn lerp(alpha: f64, p0: f64, p1: f64) -> f64 {
    p0 + alpha * (p1 - p0)
}

/// `Mth.lerp2`.
pub fn lerp2(a1: f64, a2: f64, x00: f64, x10: f64, x01: f64, x11: f64) -> f64 {
    lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

/// `Mth.lerp3`.
#[allow(clippy::too_many_arguments)]
pub fn lerp3(
    a1: f64,
    a2: f64,
    a3: f64,
    x000: f64,
    x100: f64,
    x010: f64,
    x110: f64,
    x001: f64,
    x101: f64,
    x011: f64,
    x111: f64,
) -> f64 {
    lerp(
        a3,
        lerp2(a1, a2, x000, x100, x010, x110),
        lerp2(a1, a2, x001, x101, x011, x111),
    )
}

/// `Mth.clampedLerp`.
pub fn clamped_lerp(factor: f64, min: f64, max: f64) -> f64 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        lerp(factor, min, max)
    }
}

/// `Mth.smoothstep` (the quintic 6t⁵−15t⁴+10t³).
pub fn smoothstep(x: f64) -> f64 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// One octave of 3D Perlin noise (`ImprovedNoise`): a shuffled 256-entry
/// permutation table plus per-instance offsets, all drawn from the source in
/// vanilla's exact order.
pub struct ImprovedNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl ImprovedNoise {
    pub fn new(random: &mut RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256 {
            let offset = random.next_int_bounded(256 - i as i32) as usize;
            p.swap(i, i + offset);
        }
        Self { p, xo, yo, zo }
    }

    fn p(&self, x: i32) -> i32 {
        self.p[(x & 0xFF) as usize] as i32
    }

    pub fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise_smeared(x, y, z, 0.0, 0.0)
    }

    /// The deprecated 5-arg `noise(x, y, z, yScale, yFudge)` used by
    /// `BlendedNoise` and legacy paths: quantizes the y fraction to `yScale`
    /// steps (with vanilla's float `1.0E-7F` epsilon) before sampling, while
    /// smoothing over the *original* y fraction.
    pub fn noise_smeared(&self, x: f64, y: f64, z: f64, y_scale: f64, y_fudge: f64) -> f64 {
        let x = x + self.xo;
        let y = y + self.yo;
        let z = z + self.zo;
        let xf = x.floor() as i32;
        let yf = y.floor() as i32;
        let zf = z.floor() as i32;
        let xr = x - xf as f64;
        let yr = y - yf as f64;
        let zr = z - zf as f64;
        let yr_fudge = if y_scale != 0.0 {
            let fudge_limit = if y_fudge >= 0.0 && y_fudge < yr { y_fudge } else { yr };
            (fudge_limit / y_scale + 1.0E-7_f32 as f64).floor() as i32 as f64 * y_scale
        } else {
            0.0
        };
        self.sample_and_lerp(xf, yf, zf, xr, yr - yr_fudge, zr, yr)
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(&self, x: i32, y: i32, z: i32, xr: f64, yr: f64, zr: f64, yr_original: f64) -> f64 {
        let x0 = self.p(x);
        let x1 = self.p(x + 1);
        let xy00 = self.p(x0 + y);
        let xy01 = self.p(x0 + y + 1);
        let xy10 = self.p(x1 + y);
        let xy11 = self.p(x1 + y + 1);
        let g = |hash: i32| &GRADIENT[(hash & 15) as usize];
        let d000 = dot(g(self.p(xy00 + z)), xr, yr, zr);
        let d100 = dot(g(self.p(xy10 + z)), xr - 1.0, yr, zr);
        let d010 = dot(g(self.p(xy01 + z)), xr, yr - 1.0, zr);
        let d110 = dot(g(self.p(xy11 + z)), xr - 1.0, yr - 1.0, zr);
        let d001 = dot(g(self.p(xy00 + z + 1)), xr, yr, zr - 1.0);
        let d101 = dot(g(self.p(xy10 + z + 1)), xr - 1.0, yr, zr - 1.0);
        let d011 = dot(g(self.p(xy01 + z + 1)), xr, yr - 1.0, zr - 1.0);
        let d111 = dot(g(self.p(xy11 + z + 1)), xr - 1.0, yr - 1.0, zr - 1.0);
        lerp3(
            smoothstep(xr),
            smoothstep(yr_original),
            smoothstep(zr),
            d000,
            d100,
            d010,
            d110,
            d001,
            d101,
            d011,
            d111,
        )
    }
}

/// A fractal octave stack over `ImprovedNoise` (`PerlinNoise`), with sparse
/// amplitudes indexed from `first_octave`.
pub struct PerlinNoise {
    levels: Vec<Option<ImprovedNoise>>,
    #[allow(dead_code)] // serialized parameter, read back by NoiseRouter data plumbing later.
    first_octave: i32,
    amplitudes: Vec<f64>,
    lowest_freq_input_factor: f64,
    lowest_freq_value_factor: f64,
    max_value: f64,
}

impl PerlinNoise {
    /// `PerlinNoise.create` (new initialization): each non-zero-amplitude
    /// octave gets its own source via `forkPositional().fromHashOf("octave_N")`.
    pub fn create(random: &mut RandomSource, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let positional = random.fork_positional();
        Self::build(first_octave, amplitudes, |i, first_octave, levels| {
            let octave = first_octave + i as i32;
            levels.push(Some(ImprovedNoise::new(
                &mut positional.from_hash_of(&format!("octave_{octave}")),
            )));
        })
    }

    /// `createLegacyForBlendedNoise(random, IntStream.rangeClosed(from, to))`:
    /// a contiguous all-ones octave range consuming the shared source
    /// sequentially, zero octave first, then descending.
    pub fn create_legacy_contiguous(random: &mut RandomSource, from: i32, to: i32) -> Self {
        debug_assert!(from <= to && to == 0, "legacy blended-noise ranges end at octave 0");
        let count = (to - from + 1) as usize;
        let amplitudes = vec![1.0; count];
        let zero_index = count - 1; // -firstOctave
        let mut levels: Vec<Option<ImprovedNoise>> = (0..count).map(|_| None).collect();
        levels[zero_index] = Some(ImprovedNoise::new(random));
        for i in (0..zero_index).rev() {
            // All amplitudes are 1.0 in this mode, so no 262-draw skips occur.
            levels[i] = Some(ImprovedNoise::new(random));
        }
        Self::finish(from, amplitudes, levels)
    }

    fn build(
        first_octave: i32,
        amplitudes: Vec<f64>,
        mut make: impl FnMut(usize, i32, &mut Vec<Option<ImprovedNoise>>),
    ) -> Self {
        let mut levels = Vec::with_capacity(amplitudes.len());
        for (i, &amp) in amplitudes.iter().enumerate() {
            if amp != 0.0 {
                make(i, first_octave, &mut levels);
            } else {
                levels.push(None);
            }
        }
        Self::finish(first_octave, amplitudes, levels)
    }

    fn finish(first_octave: i32, amplitudes: Vec<f64>, levels: Vec<Option<ImprovedNoise>>) -> Self {
        let octaves = amplitudes.len();
        let zero_octave_index = -first_octave;
        let lowest_freq_input_factor = 2.0f64.powi(-zero_octave_index);
        let lowest_freq_value_factor =
            2.0f64.powi(octaves as i32 - 1) / (2.0f64.powi(octaves as i32) - 1.0);
        let mut this = Self {
            levels,
            first_octave,
            amplitudes,
            lowest_freq_input_factor,
            lowest_freq_value_factor,
            max_value: 0.0,
        };
        this.max_value = this.edge_value(2.0);
        this
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    /// `maxBrokenValue` — `BlendedNoise`'s bound.
    pub fn max_broken_value(&self, y_scale: f64) -> f64 {
        self.edge_value(y_scale + 2.0)
    }

    fn edge_value(&self, noise_value: f64) -> f64 {
        let mut value = 0.0;
        let mut value_factor = self.lowest_freq_value_factor;
        for (i, level) in self.levels.iter().enumerate() {
            if level.is_some() {
                value += self.amplitudes[i] * noise_value * value_factor;
            }
            value_factor /= 2.0;
        }
        value
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        self.get_value_smeared(x, y, z, 0.0, 0.0)
    }

    /// The deprecated 5-arg `getValue` (legacy smear path).
    pub fn get_value_smeared(&self, x: f64, y: f64, z: f64, y_scale: f64, y_fudge: f64) -> f64 {
        let mut value = 0.0;
        let mut factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;
        for (i, level) in self.levels.iter().enumerate() {
            if let Some(noise) = level {
                let v = noise.noise_smeared(
                    wrap(x * factor),
                    wrap(y * factor),
                    wrap(z * factor),
                    y_scale * factor,
                    y_fudge * factor,
                );
                value += self.amplitudes[i] * v * value_factor;
            }
            factor *= 2.0;
            value_factor /= 2.0;
        }
        value
    }

    /// `getOctaveNoise(i)` — indexed from the *highest*-frequency end.
    pub fn get_octave_noise(&self, i: usize) -> Option<&ImprovedNoise> {
        self.levels[self.levels.len() - 1 - i].as_ref()
    }
}

/// `PerlinNoise.wrap`: keeps inputs inside ±2²⁵ · ~half to avoid precision
/// loss at extreme coordinates.
pub fn wrap(x: f64) -> f64 {
    x - (x / 3.3554432E7 + 0.5).floor() as i64 as f64 * 3.3554432E7
}

/// Two decorrelated `PerlinNoise` stacks averaged (`NormalNoise`) — the type
/// behind every named vanilla noise.
pub struct NormalNoise {
    value_factor: f64,
    first: PerlinNoise,
    second: PerlinNoise,
    max_value: f64,
}

/// `NormalNoise.NoiseParameters` — the datapack `worldgen/noise` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseParameters {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

const INPUT_FACTOR: f64 = 1.0181268882175227;

impl NormalNoise {
    /// `NormalNoise.create` (new initialization).
    pub fn create(random: &mut RandomSource, parameters: &NoiseParameters) -> Self {
        let first = PerlinNoise::create(random, parameters.first_octave, parameters.amplitudes.clone());
        let second = PerlinNoise::create(random, parameters.first_octave, parameters.amplitudes.clone());
        let mut min_octave = i32::MAX;
        let mut max_octave = i32::MIN;
        for (i, &amp) in parameters.amplitudes.iter().enumerate() {
            if amp != 0.0 {
                min_octave = min_octave.min(i as i32);
                max_octave = max_octave.max(i as i32);
            }
        }
        let expected_deviation = 0.1 * (1.0 + 1.0 / (max_octave - min_octave + 1) as f64);
        let value_factor = 0.16666666666666666 / expected_deviation;
        let max_value = (first.max_value() + second.max_value()) * value_factor;
        Self {
            value_factor,
            first,
            second,
            max_value,
        }
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let x2 = x * INPUT_FACTOR;
        let y2 = y * INPUT_FACTOR;
        let z2 = z * INPUT_FACTOR;
        (self.first.get_value(x, y, z) + self.second.get_value(x2, y2, z2)) * self.value_factor
    }
}

/// The legacy composite 3D terrain noise (`BlendedNoise`): an 8-octave main
/// noise selecting between two 16-octave limit noises. Still the overworld's
/// `base_3d_noise` density function.
pub struct BlendedNoise {
    min_limit_noise: PerlinNoise,
    max_limit_noise: PerlinNoise,
    main_noise: PerlinNoise,
    xz_multiplier: f64,
    y_multiplier: f64,
    xz_factor: f64,
    y_factor: f64,
    smear_scale_multiplier: f64,
    max_value: f64,
}

impl BlendedNoise {
    /// The seeded constructor. Overworld: `(0.25, 0.125, 80.0, 160.0, 8.0)`.
    pub fn new(
        random: &mut RandomSource,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let min_limit_noise = PerlinNoise::create_legacy_contiguous(random, -15, 0);
        let max_limit_noise = PerlinNoise::create_legacy_contiguous(random, -15, 0);
        let main_noise = PerlinNoise::create_legacy_contiguous(random, -7, 0);
        let xz_multiplier = 684.412 * xz_scale;
        let y_multiplier = 684.412 * y_scale;
        let max_value = min_limit_noise.max_broken_value(y_multiplier);
        Self {
            min_limit_noise,
            max_limit_noise,
            main_noise,
            xz_multiplier,
            y_multiplier,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
            max_value,
        }
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn min_value(&self) -> f64 {
        -self.max_value
    }

    /// `compute(FunctionContext)` over a block position.
    pub fn compute(&self, block_x: i32, block_y: i32, block_z: i32) -> f64 {
        let limit_x = block_x as f64 * self.xz_multiplier;
        let limit_y = block_y as f64 * self.y_multiplier;
        let limit_z = block_z as f64 * self.xz_multiplier;
        let main_x = limit_x / self.xz_factor;
        let main_y = limit_y / self.y_factor;
        let main_z = limit_z / self.xz_factor;
        let limit_smear = self.y_multiplier * self.smear_scale_multiplier;
        let main_smear = limit_smear / self.y_factor;

        let mut main_value = 0.0;
        let mut pow = 1.0;
        for i in 0..8 {
            if let Some(noise) = self.main_noise.get_octave_noise(i) {
                main_value += noise.noise_smeared(
                    wrap(main_x * pow),
                    wrap(main_y * pow),
                    wrap(main_z * pow),
                    main_smear * pow,
                    main_y * pow,
                ) / pow;
            }
            pow /= 2.0;
        }

        let factor = (main_value / 10.0 + 1.0) / 2.0;
        let is_max = factor >= 1.0;
        let is_min = factor <= 0.0;
        let mut blend_min = 0.0;
        let mut blend_max = 0.0;
        let mut pow = 1.0;
        for i in 0..16 {
            let wx = wrap(limit_x * pow);
            let wy = wrap(limit_y * pow);
            let wz = wrap(limit_z * pow);
            let y_scale_pow = limit_smear * pow;
            if !is_max {
                if let Some(noise) = self.min_limit_noise.get_octave_noise(i) {
                    blend_min += noise.noise_smeared(wx, wy, wz, y_scale_pow, limit_y * pow) / pow;
                }
            }
            if !is_min {
                if let Some(noise) = self.max_limit_noise.get_octave_noise(i) {
                    blend_max += noise.noise_smeared(wx, wy, wz, y_scale_pow, limit_y * pow) / pow;
                }
            }
            pow /= 2.0;
        }

        clamped_lerp(factor, blend_min / 512.0, blend_max / 512.0) / 128.0
    }
}

/// 2D simplex noise (`SimplexNoise`) — used only by the biome temperature
/// fields (`Biome.TEMPERATURE_NOISE` and friends), all seeded on the legacy
/// LCG. Shares [`GRADIENT`] with the Perlin implementation but hashes with a
/// `% 12` (not `& 15`), so the 4 padding rows are unreachable here.
pub struct SimplexNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

const SIMPLEX_F2: f64 = 0.3660254037844386; // 0.5 * (sqrt(3) - 1)
const SIMPLEX_G2: f64 = 0.21132486540518713; // (3 - sqrt(3)) / 6

impl SimplexNoise {
    pub fn new(random: &mut RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256 {
            let offset = random.next_int_bounded(256 - i as i32) as usize;
            p.swap(i, i + offset);
        }
        Self { p, xo, yo, zo }
    }

    fn p(&self, x: i32) -> i32 {
        self.p[(x & 0xFF) as usize] as i32
    }

    fn corner_noise(index: i32, x: f64, y: f64, z: f64, base: f64) -> f64 {
        let t = base - x * x - y * y - z * z;
        if t < 0.0 {
            0.0
        } else {
            let t = t * t;
            t * t * dot(&GRADIENT[index as usize], x, y, z)
        }
    }

    /// The 2D `getValue(x, y)`.
    pub fn get_value_2d(&self, xin: f64, yin: f64) -> f64 {
        let s = (xin + yin) * SIMPLEX_F2;
        let i = (xin + s).floor() as i32;
        let j = (yin + s).floor() as i32;
        let t = (i + j) as f64 * SIMPLEX_G2;
        let x0 = xin - (i as f64 - t);
        let y0 = yin - (j as f64 - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - i1 as f64 + SIMPLEX_G2;
        let y1 = y0 - j1 as f64 + SIMPLEX_G2;
        let x2 = x0 - 1.0 + 2.0 * SIMPLEX_G2;
        let y2 = y0 - 1.0 + 2.0 * SIMPLEX_G2;
        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gi0 = self.p(ii + self.p(jj)) % 12;
        let gi1 = self.p(ii + i1 + self.p(jj + j1)) % 12;
        let gi2 = self.p(ii + 1 + self.p(jj + 1)) % 12;
        let n0 = Self::corner_noise(gi0, x0, y0, 0.0, 0.5);
        let n1 = Self::corner_noise(gi1, x1, y1, 0.0, 0.5);
        let n2 = Self::corner_noise(gi2, x2, y2, 0.0, 0.5);
        70.0 * (n0 + n1 + n2)
    }
}

/// Octave stack over [`SimplexNoise`] (`PerlinSimplexNoise`). Only the
/// non-positive-octave sets the biome temperature fields use are supported —
/// the positive-octave reseed path (`positiveOctaveSeed`) never runs for them.
pub struct PerlinSimplexNoise {
    levels: Vec<Option<SimplexNoise>>,
    highest_freq_value_factor: f64,
    highest_freq_input_factor: f64,
}

impl PerlinSimplexNoise {
    /// `octaves` is the sorted distinct octave set (e.g. `[-2, -1, 0]`), which
    /// must end at 0 (no positive octaves).
    pub fn new(random: &mut RandomSource, octaves: &[i32]) -> Self {
        let first = octaves[0];
        let last = *octaves.last().unwrap();
        assert!(last == 0, "positive-octave PerlinSimplexNoise is unsupported");
        let low_freq_octaves = -first;
        let count = (low_freq_octaves + last + 1) as usize;
        let zero_octave = SimplexNoise::new(random);
        let zero_index = last as usize;
        let mut levels: Vec<Option<SimplexNoise>> = (0..count).map(|_| None).collect();
        if octaves.contains(&0) {
            levels[zero_index] = Some(zero_octave);
        }
        for i in zero_index + 1..count {
            if octaves.contains(&(zero_index as i32 - i as i32)) {
                levels[i] = Some(SimplexNoise::new(random));
            } else {
                random.consume_count(262);
            }
        }
        Self {
            levels,
            highest_freq_input_factor: 2.0f64.powi(last),
            highest_freq_value_factor: 1.0 / (2.0f64.powi(count as i32) - 1.0),
        }
    }

    pub fn get_value_2d(&self, x: f64, y: f64, use_noise_start: bool) -> f64 {
        let mut value = 0.0;
        let mut factor = self.highest_freq_input_factor;
        let mut value_factor = self.highest_freq_value_factor;
        for level in &self.levels {
            if let Some(noise) = level {
                let (ox, oy) = if use_noise_start { (noise.xo, noise.yo) } else { (0.0, 0.0) };
                value += noise.get_value_2d(x * factor + ox, y * factor + oy) * value_factor;
            }
            factor /= 2.0;
            value_factor *= 2.0;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden values from the JVM reference harness (Java 25).

    #[test]
    fn improved_noise_construction_and_samples() {
        let mut random = RandomSource::xoroshiro(42);
        let n = ImprovedNoise::new(&mut random);
        assert_eq!(n.xo, 190.83062484342904);
        assert_eq!(n.yo, 101.88674612737026);
        assert_eq!(n.zo, 151.323544791807);
        assert_eq!(&n.p[..8], &[123, 170, 74, 238, 106, 188, 193, 197]);
        assert_eq!(n.noise(0.5, 1.5, -2.5), -0.13665633642335107);
        assert_eq!(n.noise(123.456, -78.9, 0.001), 0.03781873531400645);
        assert_eq!(n.noise_smeared(3.25, -1.75, 9.5, 4.0, 3.0), 0.21404226578328295);
    }

    #[test]
    fn perlin_noise_new_init() {
        let mut random = RandomSource::xoroshiro(99);
        let n = PerlinNoise::create(&mut random, -6, vec![1.0, 1.0, 1.0, 1.0]);
        assert_eq!(n.max_value(), 2.0);
        assert_eq!(n.get_value(0.0, 0.0, 0.0), -0.05034808144741206);
        assert_eq!(n.get_value(100.5, -64.25, 7777.75), 0.22032123083323582);
        assert_eq!(n.get_value(-4096.2, 32.0, 15.9), 0.14148654152908313);
    }

    #[test]
    fn perlin_noise_with_amplitude_holes() {
        // temperature-shaped parameters: zero amplitudes must not consume RNG
        // in new mode and must not contribute.
        let mut random = RandomSource::xoroshiro(5);
        let n = PerlinNoise::create(&mut random, -10, vec![1.5, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(n.get_value(1000.1, 0.0, -2000.9), -0.13282729750398356);
    }

    #[test]
    fn normal_noise() {
        let mut random = RandomSource::xoroshiro(7);
        let n = NormalNoise::create(
            &mut random,
            &NoiseParameters { first_octave: -7, amplitudes: vec![1.0, 1.0, 1.0, 1.0] },
        );
        assert_eq!(n.value_factor, 1.3333333333333333);
        assert_eq!(n.max_value(), 5.333333333333333);
        assert_eq!(n.get_value(0.5, -0.5, 44.4), 0.7679395109856377);
        assert_eq!(n.get_value(-1234.5, 64.0, 987.6), 0.0774022447312113);

        let mut random = RandomSource::xoroshiro(3);
        let t = NormalNoise::create(
            &mut random,
            &NoiseParameters { first_octave: -10, amplitudes: vec![1.5, 0.0, 1.0, 0.0, 0.0, 0.0] },
        );
        assert_eq!(t.get_value(200.25, 0.0, -300.75), -0.5674443308331448);
    }

    #[test]
    fn blended_noise_overworld_params() {
        let mut random = RandomSource::xoroshiro(0);
        let n = BlendedNoise::new(&mut random, 0.25, 0.125, 80.0, 160.0, 8.0);
        assert_eq!(n.max_value(), 87.55150000000002);
        assert_eq!(n.compute(0, 0, 0), 0.05283727086562935);
        assert_eq!(n.compute(100, 37, -250), 0.22092575324676011);
        assert_eq!(n.compute(-8000, 128, 8000), -0.046431507550199896);
        assert_eq!(n.compute(16, -60, 16), -0.1431150170056804);
    }

    #[test]
    fn blended_noise_over_legacy_random() {
        // Exercises the legacy sequential construction order (zero octave
        // first, then descending) over the LCG.
        let mut random = RandomSource::legacy(12345);
        let n = BlendedNoise::new(&mut random, 0.25, 0.125, 80.0, 160.0, 8.0);
        assert_eq!(n.compute(7, 100, -7), 0.2322968520370485);
    }
}
