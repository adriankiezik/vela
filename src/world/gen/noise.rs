//! Hand-written value noise (2D and 3D) with fbm summation — the continuous
//! deterministic field primitive under the height, climate, and cave layers.
//!
//! This is intentionally *not* a port of vanilla's `PerlinNoise`/`NormalNoise`
//! octave stack — just enough smoothly-interpolated, seamless-across-chunks noise
//! to shape believable terrain. Every sample is a pure function of world
//! coordinates plus a per-field seed, so adjacent chunks line up exactly.

/// 32-bit integer hash of a 2D lattice point (xorshift-multiply avalanche).
fn hash2(x: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed;
    h ^= (x as u32).wrapping_mul(0x9E37_79B1);
    h = h.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h ^= (z as u32).wrapping_mul(0xC2B2_AE3D);
    h = h.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 13;
    h
}

/// 32-bit integer hash of a 3D lattice point.
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = hash2(x, z, seed);
    h ^= (y as u32).wrapping_mul(0x68E3_1DA4);
    h = h.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h
}

/// A hashed lattice value in `[-1, 1]`.
fn unit(h: u32) -> f64 {
    (h as f64 / u32::MAX as f64) * 2.0 - 1.0
}

/// Smoothstep (`3t² − 2t³`) for C¹-continuous interpolation between lattice points.
fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Bilinear value noise at `(x, z)` in lattice space, in `[-1, 1]`.
fn value2(x: f64, z: f64, seed: u32) -> f64 {
    let x0 = x.floor() as i32;
    let z0 = z.floor() as i32;
    let sx = smoothstep(x - x0 as f64);
    let sz = smoothstep(z - z0 as f64);
    let v00 = unit(hash2(x0, z0, seed));
    let v10 = unit(hash2(x0 + 1, z0, seed));
    let v01 = unit(hash2(x0, z0 + 1, seed));
    let v11 = unit(hash2(x0 + 1, z0 + 1, seed));
    lerp(lerp(v00, v10, sx), lerp(v01, v11, sx), sz)
}

/// Trilinear value noise at `(x, y, z)` in lattice space, in `[-1, 1]`.
fn value3(x: f64, y: f64, z: f64, seed: u32) -> f64 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let z0 = z.floor() as i32;
    let sx = smoothstep(x - x0 as f64);
    let sy = smoothstep(y - y0 as f64);
    let sz = smoothstep(z - z0 as f64);
    let c = |dx: i32, dy: i32, dz: i32| unit(hash3(x0 + dx, y0 + dy, z0 + dz, seed));
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), sx);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), sx);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), sx);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), sx);
    lerp(lerp(x00, x10, sy), lerp(x01, x11, sy), sz)
}

/// Fractional Brownian motion in 2D: `octaves` octaves of [`value2`], each
/// doubling frequency and halving amplitude, normalised back to roughly `[-1, 1]`.
pub fn fbm2(x: f64, z: f64, seed: u32, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut total = 0.0;
    for octave in 0..octaves {
        sum += amplitude * value2(x * frequency, z * frequency, seed ^ (octave + 1));
        total += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / total
}

/// A single 3D value-noise sample in `[-1, 1]`, used by the cave carver.
pub fn noise3(x: f64, y: f64, z: f64, seed: u32) -> f64 {
    value3(x, y, z, seed)
}
