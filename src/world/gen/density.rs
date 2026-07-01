//! Vanilla-parity density-function engine (`DensityFunctions`, `NoiseChunk`,
//! the overworld `NoiseRouter`) — P2 of docs/WORLDGEN_PARITY.md.
//!
//! The graph is built data-driven from the embedded vanilla datapack JSON
//! (`vanilla_jsons`): `noise_settings/overworld.json` is the serialized router
//! (what `NoiseRouterData` builds in code), `density_function/*` are the named
//! subgraphs it references, and `noise/*` are the `NormalNoise` parameters.
//! Building from data sidesteps transcribing ~600 lines of constants.
//!
//! Semantics are bit-for-bit with the reference:
//! - node min/max propagation matches `DensityFunctions` exactly (including
//!   the `abs`/`square` lower-bound quirk and constant folding to `MulOrAdd`),
//!   because `min`/`max` short-circuit evaluation and feed spline bounds;
//! - `CubicSpline` evaluates in `f32` like the Java original;
//! - the per-chunk cache wrappers (`Interpolated`, `FlatCache`, `Cache2D`,
//!   `CacheOnce`, `CacheAllInCell`) replicate `NoiseChunk`'s counters, slice
//!   swapping, and fill-array paths — `FlatCache`'s quart-resolution sampling
//!   *changes values*, so cache semantics are part of the spec;
//! - the chunk fill loop is `NoiseBasedChunkGenerator.doFill` (X → Z →
//!   Y-descending cells, then yInCell↓, xInCell, zInCell).
//!
//! Aquifers and ore veins are P3: this engine runs with the disabled aquifer
//! (`Aquifer.createDisabled`), which vanilla itself uses when
//! `aquifers_enabled` is false — golden tests compare against a JVM run with
//! the same settings. Blending (`blend_alpha`/`blend_offset`/`blend_density`)
//! takes the empty-blender path, which is exact for fresh worlds.

// Consumed by chunk generation once P5 (surface rules) makes parity terrain
// playable; until then only the tests exercise this module.
#![allow(dead_code)]

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use super::random::PositionalRandomFactory;
use super::synth::{lerp3, BlendedNoise, NoiseParameters, NormalNoise};
use super::vanilla_jsons;

// ---------------------------------------------------------------------------
// Math helpers (Mth)
// ---------------------------------------------------------------------------

/// `Mth.clamp(double)`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `Mth.clampedMap(double)`.
fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + factor * (to_max - to_min)
    }
}

/// `Mth.floor(double)`.
fn mth_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 { i - 1 } else { i }
}

/// `Mth.lerp(float)`.
fn lerp_f32(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

/// `ChunkPos.pack` / `ColumnPos.asLong`.
fn pack_2d(x: i32, z: i32) -> i64 {
    (x as u32 as i64) | ((z as u32 as i64) << 32)
}

/// `ChunkPos.INVALID_CHUNK_POS` — the vanilla "no cached position" sentinel.
const INVALID_POS_2D: i64 = (1875066u32 as i64) | ((1875066u32 as i64) << 32);

/// `QuartPos.fromBlock` / `toBlock`.
fn quart_from_block(b: i32) -> i32 {
    b >> 2
}
fn quart_to_block(q: i32) -> i32 {
    q << 2
}

// ---------------------------------------------------------------------------
// Vanilla datapack data
// ---------------------------------------------------------------------------

/// The embedded `worldgen/*` registries plus the parsed overworld settings.
pub struct VanillaWorldgenData {
    noise_params: HashMap<&'static str, NoiseParameters>,
    density_fns: HashMap<&'static str, Value>,
    pub settings: NoiseGeneratorSettings,
}

/// `NoiseGeneratorSettings` — the parts P2 needs from
/// `noise_settings/overworld.json`.
pub struct NoiseGeneratorSettings {
    pub noise: NoiseSettings,
    pub sea_level: i32,
    pub legacy_random_source: bool,
    pub aquifers_enabled: bool,
    pub ore_veins_enabled: bool,
    /// The 15 raw router graphs, keyed by their JSON field name.
    noise_router: Value,
}

/// `NoiseSettings` (overworld: −64, 384, 1, 2 → 4×8 cells).
#[derive(Clone, Copy)]
pub struct NoiseSettings {
    pub min_y: i32,
    pub height: i32,
    pub size_horizontal: i32,
    pub size_vertical: i32,
}

impl NoiseSettings {
    pub fn cell_width(self) -> i32 {
        quart_to_block(self.size_horizontal)
    }
    pub fn cell_height(self) -> i32 {
        quart_to_block(self.size_vertical)
    }
}

impl VanillaWorldgenData {
    pub fn load_overworld() -> Self {
        let mut noise_params = HashMap::new();
        for &(id, json) in vanilla_jsons::NOISE_PARAMS {
            let v: Value = serde_json::from_str(json).expect("bad noise JSON");
            noise_params.insert(
                id,
                NoiseParameters {
                    first_octave: v["firstOctave"].as_i64().expect("firstOctave") as i32,
                    amplitudes: v["amplitudes"]
                        .as_array()
                        .expect("amplitudes")
                        .iter()
                        .map(|a| a.as_f64().expect("amplitude"))
                        .collect(),
                },
            );
        }
        let mut density_fns = HashMap::new();
        for &(id, json) in vanilla_jsons::DENSITY_FUNCTIONS {
            density_fns.insert(id, serde_json::from_str(json).expect("bad density JSON"));
        }
        let s: Value =
            serde_json::from_str(vanilla_jsons::OVERWORLD_NOISE_SETTINGS).expect("bad settings");
        let n = &s["noise"];
        let settings = NoiseGeneratorSettings {
            noise: NoiseSettings {
                min_y: n["min_y"].as_i64().unwrap() as i32,
                height: n["height"].as_i64().unwrap() as i32,
                size_horizontal: n["size_horizontal"].as_i64().unwrap() as i32,
                size_vertical: n["size_vertical"].as_i64().unwrap() as i32,
            },
            sea_level: s["sea_level"].as_i64().unwrap() as i32,
            legacy_random_source: s["legacy_random_source"].as_bool().unwrap(),
            aquifers_enabled: s["aquifers_enabled"].as_bool().unwrap(),
            ore_veins_enabled: s["ore_veins_enabled"].as_bool().unwrap(),
            noise_router: s["noise_router"].clone(),
        };
        Self { noise_params, density_fns, settings }
    }
}

// ---------------------------------------------------------------------------
// The density-function node graph
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkerKind {
    Interpolated,
    FlatCache,
    Cache2D,
    CacheOnce,
    CacheAllInCell,
    BlendDensity,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MappedKind {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Invert,
    Squeeze,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ap2Kind {
    Add,
    Mul,
    Min,
    Max,
}

/// One density-function node. The seeded router graph uses the `Marker`
/// variant (compute-through, like vanilla's `DensityFunctions.Marker`); a
/// `NoiseChunk` rewrites markers into the `*Cache`/`Interp` variants whose
/// `slot` indexes that chunk's mutable state.
pub enum Dfn {
    Constant(f64),
    YClampedGradient { from_y: i32, to_y: i32, from_value: f64, to_value: f64 },
    Noise { noise: Rc<NormalNoise>, xz_scale: f64, y_scale: f64 },
    ShiftA(Rc<NormalNoise>),
    ShiftB(Rc<NormalNoise>),
    Shift(Rc<NormalNoise>),
    ShiftedNoise {
        shift_x: Rc<Dfn>,
        shift_y: Rc<Dfn>,
        shift_z: Rc<Dfn>,
        xz_scale: f64,
        y_scale: f64,
        noise: Rc<NormalNoise>,
    },
    BlendedNoise(Rc<BlendedNoise>),
    BlendAlpha,
    BlendOffset,
    /// `BeardifierMarker` — the no-op structure-terrain contribution (exactly
    /// vanilla wherever no adapting structure is nearby; structures are P9).
    Beardifier,
    Marker { kind: MarkerKind, wrapped: Rc<Dfn> },
    Clamp { input: Rc<Dfn>, min: f64, max: f64 },
    Mapped { kind: MappedKind, input: Rc<Dfn>, min: f64, max: f64 },
    Ap2 { kind: Ap2Kind, argument1: Rc<Dfn>, argument2: Rc<Dfn>, min: f64, max: f64 },
    /// The `add`/`mul` constant-folded form.
    MulOrAdd { is_mul: bool, input: Rc<Dfn>, argument: f64, min: f64, max: f64 },
    RangeChoice {
        input: Rc<Dfn>,
        min_inclusive: f64,
        max_exclusive: f64,
        when_in_range: Rc<Dfn>,
        when_out_of_range: Rc<Dfn>,
    },
    IntervalSelect { input: Rc<Dfn>, thresholds: Vec<f64>, functions: Vec<Rc<Dfn>> },
    Spline(Rc<Spline>),
    FindTopSurface { density: Rc<Dfn>, upper_bound: Rc<Dfn>, lower_bound: i32, cell_height: i32 },
    // --- chunk-local cache nodes (only in NoiseChunk-wrapped graphs) ---
    Interp { slot: usize, filler: Rc<Dfn> },
    FlatCacheNode { slot: usize, filler: Rc<Dfn> },
    Cache2DNode { slot: usize, filler: Rc<Dfn> },
    CacheOnceNode { slot: usize, filler: Rc<Dfn> },
    CacheCellNode { slot: usize, filler: Rc<Dfn> },
}

impl Dfn {
    pub fn min_value(&self) -> f64 {
        match self {
            Dfn::Constant(v) => *v,
            Dfn::YClampedGradient { from_value, to_value, .. } => from_value.min(*to_value),
            Dfn::Noise { noise, .. } => -noise.max_value(),
            Dfn::ShiftA(n) | Dfn::ShiftB(n) | Dfn::Shift(n) => -n.max_value() * 4.0,
            Dfn::ShiftedNoise { noise, .. } => -noise.max_value(),
            Dfn::BlendedNoise(n) => n.min_value(),
            Dfn::BlendAlpha => 1.0,
            Dfn::BlendOffset => 0.0,
            Dfn::Beardifier => 0.0,
            Dfn::Marker { kind: MarkerKind::BlendDensity, .. } => f64::NEG_INFINITY,
            Dfn::Marker { wrapped, .. } => wrapped.min_value(),
            Dfn::Clamp { min, .. } => *min,
            Dfn::Mapped { min, .. } => *min,
            Dfn::Ap2 { min, .. } => *min,
            Dfn::MulOrAdd { min, .. } => *min,
            Dfn::RangeChoice { when_in_range, when_out_of_range, .. } => {
                when_in_range.min_value().min(when_out_of_range.min_value())
            }
            Dfn::IntervalSelect { functions, .. } => {
                // Java folds from Double.MAX_VALUE (not +∞) — mirrored.
                functions.iter().fold(f64::MAX, |m, f| m.min(f.min_value()))
            }
            Dfn::Spline(s) => s.min_value() as f64,
            Dfn::FindTopSurface { lower_bound, .. } => *lower_bound as f64,
            Dfn::Interp { filler, .. }
            | Dfn::FlatCacheNode { filler, .. }
            | Dfn::Cache2DNode { filler, .. }
            | Dfn::CacheOnceNode { filler, .. }
            | Dfn::CacheCellNode { filler, .. } => filler.min_value(),
        }
    }

    pub fn max_value(&self) -> f64 {
        match self {
            Dfn::Constant(v) => *v,
            Dfn::YClampedGradient { from_value, to_value, .. } => from_value.max(*to_value),
            Dfn::Noise { noise, .. } => noise.max_value(),
            Dfn::ShiftA(n) | Dfn::ShiftB(n) | Dfn::Shift(n) => n.max_value() * 4.0,
            Dfn::ShiftedNoise { noise, .. } => noise.max_value(),
            Dfn::BlendedNoise(n) => n.max_value(),
            Dfn::BlendAlpha => 1.0,
            Dfn::BlendOffset => 0.0,
            Dfn::Beardifier => 0.0,
            Dfn::Marker { kind: MarkerKind::BlendDensity, .. } => f64::INFINITY,
            Dfn::Marker { wrapped, .. } => wrapped.max_value(),
            Dfn::Clamp { max, .. } => *max,
            Dfn::Mapped { max, .. } => *max,
            Dfn::Ap2 { max, .. } => *max,
            Dfn::MulOrAdd { max, .. } => *max,
            Dfn::RangeChoice { when_in_range, when_out_of_range, .. } => {
                when_in_range.max_value().max(when_out_of_range.max_value())
            }
            Dfn::IntervalSelect { functions, .. } => {
                functions.iter().fold(-f64::MAX, |m, f| m.max(f.max_value()))
            }
            Dfn::Spline(s) => s.max_value() as f64,
            Dfn::FindTopSurface { lower_bound, upper_bound, .. } => {
                (*lower_bound as f64).max(upper_bound.max_value())
            }
            Dfn::Interp { filler, .. }
            | Dfn::FlatCacheNode { filler, .. }
            | Dfn::Cache2DNode { filler, .. }
            | Dfn::CacheOnceNode { filler, .. }
            | Dfn::CacheCellNode { filler, .. } => filler.max_value(),
        }
    }
}

/// `DensityFunctions.Mapped.transform`.
fn mapped_transform(kind: MappedKind, input: f64) -> f64 {
    match kind {
        MappedKind::Abs => input.abs(),
        MappedKind::Square => input * input,
        MappedKind::Cube => input * input * input,
        MappedKind::HalfNegative => {
            if input > 0.0 { input } else { input * 0.5 }
        }
        MappedKind::QuarterNegative => {
            if input > 0.0 { input } else { input * 0.25 }
        }
        MappedKind::Invert => 1.0 / input,
        MappedKind::Squeeze => {
            let c = clamp(input, -1.0, 1.0);
            c / 2.0 - c * c * c / 24.0
        }
    }
}

/// `DensityFunctions.Mapped.create` — min/max propagation quirks included
/// (`abs`/`square` keep `max(0, input.min)` as their lower bound; `invert`
/// over a zero-spanning range is unbounded).
fn new_mapped(kind: MappedKind, input: Rc<Dfn>) -> Dfn {
    let min_value = input.min_value();
    let max_value = input.max_value();
    let min_image = mapped_transform(kind, min_value);
    let max_image = mapped_transform(kind, max_value);
    let (min, max) = if kind == MappedKind::Invert {
        if min_value < 0.0 && max_value > 0.0 {
            (f64::NEG_INFINITY, f64::INFINITY)
        } else {
            (max_image, min_image)
        }
    } else if kind == MappedKind::Abs || kind == MappedKind::Square {
        (0.0f64.max(min_value), min_image.max(max_image))
    } else {
        (min_image, max_image)
    };
    Dfn::Mapped { kind, input, min, max }
}

/// `TwoArgumentSimpleFunction.create` — folds a constant argument into
/// `MulOrAdd` for `add`/`mul`, and computes the interval bounds.
fn new_ap2(kind: Ap2Kind, argument1: Rc<Dfn>, argument2: Rc<Dfn>) -> Dfn {
    let min1 = argument1.min_value();
    let min2 = argument2.min_value();
    let max1 = argument1.max_value();
    let max2 = argument2.max_value();
    let min = match kind {
        Ap2Kind::Add => min1 + min2,
        Ap2Kind::Mul => {
            if min1 > 0.0 && min2 > 0.0 {
                min1 * min2
            } else if max1 < 0.0 && max2 < 0.0 {
                max1 * max2
            } else {
                (min1 * max2).min(max1 * min2)
            }
        }
        Ap2Kind::Min => min1.min(min2),
        Ap2Kind::Max => min1.max(min2),
    };
    let max = match kind {
        Ap2Kind::Add => max1 + max2,
        Ap2Kind::Mul => {
            if min1 > 0.0 && min2 > 0.0 {
                max1 * max2
            } else if max1 < 0.0 && max2 < 0.0 {
                min1 * min2
            } else {
                (min1 * min2).max(max1 * max2)
            }
        }
        Ap2Kind::Min => max1.min(max2),
        Ap2Kind::Max => max1.max(max2),
    };
    if kind == Ap2Kind::Mul || kind == Ap2Kind::Add {
        let is_mul = kind == Ap2Kind::Mul;
        if let Dfn::Constant(c) = *argument1 {
            return Dfn::MulOrAdd { is_mul, input: argument2, argument: c, min, max };
        }
        if let Dfn::Constant(c) = *argument2 {
            return Dfn::MulOrAdd { is_mul, input: argument1, argument: c, min, max };
        }
    }
    Dfn::Ap2 { kind, argument1, argument2, min, max }
}

// ---------------------------------------------------------------------------
// CubicSpline (util/CubicSpline.java) — all arithmetic in f32
// ---------------------------------------------------------------------------

pub enum Spline {
    Constant(f32),
    Multipoint {
        coordinate: Rc<Dfn>,
        locations: Vec<f32>,
        values: Vec<Spline>,
        derivatives: Vec<f32>,
        min: f32,
        max: f32,
    },
}

impl Spline {
    pub fn min_value(&self) -> f32 {
        match self {
            Spline::Constant(v) => *v,
            Spline::Multipoint { min, .. } => *min,
        }
    }

    pub fn max_value(&self) -> f32 {
        match self {
            Spline::Constant(v) => *v,
            Spline::Multipoint { max, .. } => *max,
        }
    }

    /// `CubicSpline.Multipoint::new` — the min/max estimation over knot
    /// intervals, replicated in float.
    fn new_multipoint(
        coordinate: Rc<Dfn>,
        locations: Vec<f32>,
        values: Vec<Spline>,
        derivatives: Vec<f32>,
    ) -> Spline {
        assert!(
            locations.len() == values.len() && locations.len() == derivatives.len(),
            "spline knot arrays must be equal length"
        );
        assert!(!locations.is_empty(), "spline needs at least one point");
        let last_index = locations.len() - 1;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let min_input = coordinate.min_value() as f32;
        let max_input = coordinate.max_value() as f32;
        if min_input < locations[0] {
            let edge1 = linear_extend(min_input, &locations, values[0].min_value(), &derivatives, 0);
            let edge2 = linear_extend(min_input, &locations, values[0].max_value(), &derivatives, 0);
            min = min.min(edge1.min(edge2));
            max = max.max(edge1.max(edge2));
        }
        if max_input > locations[last_index] {
            let edge1 = linear_extend(
                max_input,
                &locations,
                values[last_index].min_value(),
                &derivatives,
                last_index,
            );
            let edge2 = linear_extend(
                max_input,
                &locations,
                values[last_index].max_value(),
                &derivatives,
                last_index,
            );
            min = min.min(edge1.min(edge2));
            max = max.max(edge1.max(edge2));
        }
        for value in &values {
            min = min.min(value.min_value());
            max = max.max(value.max_value());
        }
        for i in 0..last_index {
            let x1 = locations[i];
            let x2 = locations[i + 1];
            let x_diff = x2 - x1;
            let v1 = &values[i];
            let v2 = &values[i + 1];
            let min1 = v1.min_value();
            let max1 = v1.max_value();
            let min2 = v2.min_value();
            let max2 = v2.max_value();
            let d1 = derivatives[i];
            let d2 = derivatives[i + 1];
            if d1 != 0.0 || d2 != 0.0 {
                let p1 = d1 * x_diff;
                let p2 = d2 * x_diff;
                let min_lerp1 = min1.min(min2);
                let max_lerp1 = max1.max(max2);
                let min_a = p1 - max2 + min1;
                let max_a = p1 - min2 + max1;
                let min_b = -p2 + min2 - max1;
                let max_b = -p2 + max2 - min1;
                let min_lerp2 = min_a.min(min_b);
                let max_lerp2 = max_a.max(max_b);
                min = min.min(min_lerp1 + 0.25 * min_lerp2);
                max = max.max(max_lerp1 + 0.25 * max_lerp2);
            }
        }
        Spline::Multipoint { coordinate, locations, values, derivatives, min, max }
    }
}

/// `CubicSpline.Multipoint.linearExtend`.
fn linear_extend(input: f32, locations: &[f32], value: f32, derivatives: &[f32], index: usize) -> f32 {
    let derivative = derivatives[index];
    if derivative == 0.0 { value } else { value + derivative * (input - locations[index]) }
}

/// `Mth.binarySearch(0, len, i -> input < locations[i]) - 1`.
fn find_interval_start(locations: &[f32], input: f32) -> isize {
    let mut from = 0usize;
    let mut len = locations.len();
    while len > 0 {
        let half = len / 2;
        let middle = from + half;
        if input < locations[middle] {
            len = half;
        } else {
            from = middle + 1;
            len -= half + 1;
        }
    }
    from as isize - 1
}

// ---------------------------------------------------------------------------
// Graph building from JSON (the codec layer)
// ---------------------------------------------------------------------------

/// Builds seeded runtime graphs: performs `RandomState`'s noise wiring while
/// parsing (named noises via `fromHashOf("minecraft:<id>")`, `old_blended_noise`
/// via `fromHashOf("minecraft:terrain")`), so the result is what vanilla's
/// `settings.noiseRouter().mapAll(new NoiseWiringHelper())` produces.
struct GraphBuilder<'a> {
    data: &'a VanillaWorldgenData,
    random: PositionalRandomFactory,
    noise_cache: HashMap<String, Rc<NormalNoise>>,
    ref_cache: HashMap<String, Rc<Dfn>>,
}

impl<'a> GraphBuilder<'a> {
    fn get_or_create_noise(&mut self, full_id: &str) -> Rc<NormalNoise> {
        if let Some(n) = self.noise_cache.get(full_id) {
            return n.clone();
        }
        let short = full_id.strip_prefix("minecraft:").unwrap_or(full_id);
        let params = self
            .data
            .noise_params
            .get(short)
            .unwrap_or_else(|| panic!("unknown noise parameters {full_id}"));
        let noise = Rc::new(NormalNoise::create(&mut self.random.from_hash_of(full_id), params));
        self.noise_cache.insert(full_id.to_string(), noise.clone());
        noise
    }

    fn build_noise_holder(&mut self, v: &Value) -> Rc<NormalNoise> {
        let id = v
            .as_str()
            .expect("inline noise parameters are unsupported (vanilla data always references)");
        self.get_or_create_noise(id)
    }

    fn build(&mut self, v: &Value) -> Rc<Dfn> {
        match v {
            Value::Number(n) => Rc::new(Dfn::Constant(n.as_f64().expect("constant"))),
            Value::String(id) => {
                if let Some(cached) = self.ref_cache.get(id.as_str()) {
                    return cached.clone();
                }
                let short = id.strip_prefix("minecraft:").unwrap_or(id);
                let json = self
                    .data
                    .density_fns
                    .get(short)
                    .unwrap_or_else(|| panic!("unknown density function {id}"))
                    .clone();
                let built = self.build(&json);
                self.ref_cache.insert(id.clone(), built.clone());
                built
            }
            Value::Object(_) => Rc::new(self.build_typed(v)),
            _ => panic!("unexpected density function JSON: {v}"),
        }
    }

    fn build_typed(&mut self, v: &Value) -> Dfn {
        let ty = v["type"].as_str().expect("type");
        let ty = ty.strip_prefix("minecraft:").unwrap_or(ty);
        let f = |b: &mut Self, key: &str| b.build(&v[key]);
        match ty {
            "constant" => Dfn::Constant(v["argument"].as_f64().expect("argument")),
            "y_clamped_gradient" => Dfn::YClampedGradient {
                from_y: v["from_y"].as_i64().unwrap() as i32,
                to_y: v["to_y"].as_i64().unwrap() as i32,
                from_value: v["from_value"].as_f64().unwrap(),
                to_value: v["to_value"].as_f64().unwrap(),
            },
            "noise" => Dfn::Noise {
                noise: self.build_noise_holder(&v["noise"]),
                xz_scale: v["xz_scale"].as_f64().unwrap(),
                y_scale: v["y_scale"].as_f64().unwrap(),
            },
            "shift_a" => Dfn::ShiftA(self.build_noise_holder(&v["argument"])),
            "shift_b" => Dfn::ShiftB(self.build_noise_holder(&v["argument"])),
            "shift" => Dfn::Shift(self.build_noise_holder(&v["argument"])),
            "shifted_noise" => Dfn::ShiftedNoise {
                shift_x: f(self, "shift_x"),
                shift_y: f(self, "shift_y"),
                shift_z: f(self, "shift_z"),
                xz_scale: v["xz_scale"].as_f64().unwrap(),
                y_scale: v["y_scale"].as_f64().unwrap(),
                noise: self.build_noise_holder(&v["noise"]),
            },
            "old_blended_noise" => {
                // NoiseWiringHelper.wrapNew: re-seeded from
                // fromHashOf("minecraft:terrain") (non-legacy worlds).
                let mut random = self.random.from_hash_of("minecraft:terrain");
                Dfn::BlendedNoise(Rc::new(BlendedNoise::new(
                    &mut random,
                    v["xz_scale"].as_f64().unwrap(),
                    v["y_scale"].as_f64().unwrap(),
                    v["xz_factor"].as_f64().unwrap(),
                    v["y_factor"].as_f64().unwrap(),
                    v["smear_scale_multiplier"].as_f64().unwrap(),
                )))
            }
            "blend_alpha" => Dfn::BlendAlpha,
            "blend_offset" => Dfn::BlendOffset,
            "beardifier" => Dfn::Beardifier,
            "interpolated" | "flat_cache" | "cache_2d" | "cache_once" | "cache_all_in_cell"
            | "blend_density" => Dfn::Marker {
                kind: match ty {
                    "interpolated" => MarkerKind::Interpolated,
                    "flat_cache" => MarkerKind::FlatCache,
                    "cache_2d" => MarkerKind::Cache2D,
                    "cache_once" => MarkerKind::CacheOnce,
                    "cache_all_in_cell" => MarkerKind::CacheAllInCell,
                    _ => MarkerKind::BlendDensity,
                },
                wrapped: f(self, "argument"),
            },
            "clamp" => Dfn::Clamp {
                input: f(self, "input"),
                min: v["min"].as_f64().unwrap(),
                max: v["max"].as_f64().unwrap(),
            },
            "abs" | "square" | "cube" | "half_negative" | "quarter_negative" | "invert"
            | "squeeze" => new_mapped(
                match ty {
                    "abs" => MappedKind::Abs,
                    "square" => MappedKind::Square,
                    "cube" => MappedKind::Cube,
                    "half_negative" => MappedKind::HalfNegative,
                    "quarter_negative" => MappedKind::QuarterNegative,
                    "invert" => MappedKind::Invert,
                    _ => MappedKind::Squeeze,
                },
                f(self, "argument"),
            ),
            "add" | "mul" | "min" | "max" => new_ap2(
                match ty {
                    "add" => Ap2Kind::Add,
                    "mul" => Ap2Kind::Mul,
                    "min" => Ap2Kind::Min,
                    _ => Ap2Kind::Max,
                },
                f(self, "argument1"),
                f(self, "argument2"),
            ),
            "range_choice" => Dfn::RangeChoice {
                input: f(self, "input"),
                min_inclusive: v["min_inclusive"].as_f64().unwrap(),
                max_exclusive: v["max_exclusive"].as_f64().unwrap(),
                when_in_range: f(self, "when_in_range"),
                when_out_of_range: f(self, "when_out_of_range"),
            },
            "interval_select" => Dfn::IntervalSelect {
                input: f(self, "input"),
                thresholds: v["thresholds"]
                    .as_array()
                    .expect("thresholds")
                    .iter()
                    .map(|t| t.as_f64().unwrap())
                    .collect(),
                functions: v["functions"]
                    .as_array()
                    .expect("functions")
                    .iter()
                    .map(|fj| self.build(fj))
                    .collect(),
            },
            "spline" => Dfn::Spline(Rc::new(self.build_spline(&v["spline"]))),
            "find_top_surface" => Dfn::FindTopSurface {
                density: f(self, "density"),
                upper_bound: f(self, "upper_bound"),
                lower_bound: v["lower_bound"].as_i64().unwrap() as i32,
                cell_height: v["cell_height"].as_i64().unwrap() as i32,
            },
            "end_islands" => panic!("end_islands is out of scope until the End dimension"),
            other => panic!("unsupported density function type {other}"),
        }
    }

    fn build_spline(&mut self, v: &Value) -> Spline {
        if let Some(n) = v.as_f64() {
            return Spline::Constant(n as f32);
        }
        let coordinate = self.build(&v["coordinate"]);
        let points = v["points"].as_array().expect("spline points");
        let mut locations = Vec::with_capacity(points.len());
        let mut values = Vec::with_capacity(points.len());
        let mut derivatives = Vec::with_capacity(points.len());
        for p in points {
            locations.push(p["location"].as_f64().expect("location") as f32);
            values.push(self.build_spline(&p["value"]));
            derivatives.push(p["derivative"].as_f64().expect("derivative") as f32);
        }
        Spline::new_multipoint(coordinate, locations, values, derivatives)
    }
}

// ---------------------------------------------------------------------------
// RandomState + NoiseRouter
// ---------------------------------------------------------------------------

/// The 15 seeded router outputs (`NoiseRouter`).
pub struct NoiseRouter {
    pub barrier: Rc<Dfn>,
    pub fluid_level_floodedness: Rc<Dfn>,
    pub fluid_level_spread: Rc<Dfn>,
    pub lava: Rc<Dfn>,
    pub temperature: Rc<Dfn>,
    pub vegetation: Rc<Dfn>,
    pub continents: Rc<Dfn>,
    pub erosion: Rc<Dfn>,
    pub depth: Rc<Dfn>,
    pub ridges: Rc<Dfn>,
    pub preliminary_surface_level: Rc<Dfn>,
    pub final_density: Rc<Dfn>,
    pub vein_toggle: Rc<Dfn>,
    pub vein_ridged: Rc<Dfn>,
    pub vein_gap: Rc<Dfn>,
}

/// Per-world seeded worldgen state (`RandomState`): the seeded router plus the
/// positional factories downstream layers fork from.
pub struct RandomState {
    pub router: NoiseRouter,
    pub aquifer_random: PositionalRandomFactory,
    pub ore_random: PositionalRandomFactory,
    pub settings: NoiseGeneratorSettingsPublic,
}

/// The scalar settings `NoiseChunk` needs (copied out of the parsed JSON).
#[derive(Clone, Copy)]
pub struct NoiseGeneratorSettingsPublic {
    pub noise: NoiseSettings,
    pub sea_level: i32,
    pub aquifers_enabled: bool,
    pub ore_veins_enabled: bool,
}

impl RandomState {
    pub fn new_overworld(data: &VanillaWorldgenData, seed: i64) -> Self {
        assert!(
            !data.settings.legacy_random_source,
            "legacy random source worlds are out of scope"
        );
        let mut root = super::random::RandomSource::xoroshiro(seed);
        let random = root.fork_positional();
        let aquifer_random = random.from_hash_of("minecraft:aquifer").fork_positional();
        let ore_random = random.from_hash_of("minecraft:ore").fork_positional();
        let mut b = GraphBuilder {
            data,
            random,
            noise_cache: HashMap::new(),
            ref_cache: HashMap::new(),
        };
        let r = &data.settings.noise_router;
        let field = |b: &mut GraphBuilder, name: &str| {
            assert!(!r[name].is_null(), "noise_router missing field {name}");
            b.build(&r[name])
        };
        let router = NoiseRouter {
            barrier: field(&mut b, "barrier"),
            fluid_level_floodedness: field(&mut b, "fluid_level_floodedness"),
            fluid_level_spread: field(&mut b, "fluid_level_spread"),
            lava: field(&mut b, "lava"),
            temperature: field(&mut b, "temperature"),
            vegetation: field(&mut b, "vegetation"),
            continents: field(&mut b, "continents"),
            erosion: field(&mut b, "erosion"),
            depth: field(&mut b, "depth"),
            ridges: field(&mut b, "ridges"),
            preliminary_surface_level: field(&mut b, "preliminary_surface_level"),
            final_density: field(&mut b, "final_density"),
            vein_toggle: field(&mut b, "vein_toggle"),
            vein_ridged: field(&mut b, "vein_ridged"),
            vein_gap: field(&mut b, "vein_gap"),
        };
        Self {
            router,
            aquifer_random,
            ore_random,
            settings: NoiseGeneratorSettingsPublic {
                noise: data.settings.noise,
                sea_level: data.settings.sea_level,
                aquifers_enabled: data.settings.aquifers_enabled,
                ore_veins_enabled: data.settings.ore_veins_enabled,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// A `FunctionContext`: either an explicit position (`SinglePointContext`) or
/// the `NoiseChunk` cursor (vanilla passes the chunk itself; the identity
/// check `context != NoiseChunk.this` becomes a variant check here).
#[derive(Clone, Copy)]
pub enum Ctx {
    Point { x: i32, y: i32, z: i32 },
    Cursor,
}

impl Ctx {
    fn block_x(self, st: &ChunkEvalState) -> i32 {
        match self {
            Ctx::Point { x, .. } => x,
            Ctx::Cursor => st.cell_start_block_x + st.in_cell_x,
        }
    }
    fn block_y(self, st: &ChunkEvalState) -> i32 {
        match self {
            Ctx::Point { y, .. } => y,
            Ctx::Cursor => st.cell_start_block_y + st.in_cell_y,
        }
    }
    fn block_z(self, st: &ChunkEvalState) -> i32 {
        match self {
            Ctx::Point { z, .. } => z,
            Ctx::Cursor => st.cell_start_block_z + st.in_cell_z,
        }
    }
}

/// The mutable per-chunk evaluation state: the interpolation cursor plus the
/// storage behind every cache slot. A zeroed instance backs standalone
/// (single-point) evaluation of unwrapped graphs.
pub struct ChunkEvalState {
    cell_width: i32,
    cell_height: i32,
    cell_count_xz: i32,
    cell_count_y: i32,
    cell_noise_min_y: i32,
    first_cell_x: i32,
    first_cell_z: i32,
    first_noise_x: i32,
    first_noise_z: i32,
    noise_size_xz: i32,
    interpolating: bool,
    filling_cell: bool,
    cell_start_block_x: i32,
    cell_start_block_y: i32,
    cell_start_block_z: i32,
    in_cell_x: i32,
    in_cell_y: i32,
    in_cell_z: i32,
    interpolation_counter: i64,
    array_interpolation_counter: i64,
    array_index: usize,
    interp: Vec<InterpState>,
    flat: Vec<FlatState>,
    c2d: Vec<Cache2DState>,
    once: Vec<CacheOnceState>,
    cell: Vec<Vec<f64>>,
}

impl ChunkEvalState {
    /// State for evaluating unwrapped graphs (no cache nodes) at explicit
    /// positions — what vanilla does when computing `RandomState`'s router
    /// with a `SinglePointContext`.
    pub fn standalone() -> Self {
        Self {
            cell_width: 0,
            cell_height: 0,
            cell_count_xz: 0,
            cell_count_y: 0,
            cell_noise_min_y: 0,
            first_cell_x: 0,
            first_cell_z: 0,
            first_noise_x: 0,
            first_noise_z: 0,
            noise_size_xz: 0,
            interpolating: false,
            filling_cell: false,
            cell_start_block_x: 0,
            cell_start_block_y: 0,
            cell_start_block_z: 0,
            in_cell_x: 0,
            in_cell_y: 0,
            in_cell_z: 0,
            interpolation_counter: 0,
            array_interpolation_counter: 0,
            array_index: 0,
            interp: Vec::new(),
            flat: Vec::new(),
            c2d: Vec::new(),
            once: Vec::new(),
            cell: Vec::new(),
        }
    }
}

/// `NoiseChunk.NoiseInterpolator` state: two sliding YZ slices plus the
/// running trilinear-lerp registers.
struct InterpState {
    /// `[cell_z][cell_y]`, `(cellCountXZ + 1) × (cellCountY + 1)`.
    slice0: Vec<Vec<f64>>,
    slice1: Vec<Vec<f64>>,
    noise000: f64,
    noise001: f64,
    noise100: f64,
    noise101: f64,
    noise010: f64,
    noise011: f64,
    noise110: f64,
    noise111: f64,
    value_xz00: f64,
    value_xz10: f64,
    value_xz01: f64,
    value_xz11: f64,
    value_z0: f64,
    value_z1: f64,
    value: f64,
}

impl InterpState {
    fn new(cell_count_y: i32, cell_count_xz: i32) -> Self {
        let alloc = || vec![vec![0.0; cell_count_y as usize + 1]; cell_count_xz as usize + 1];
        Self {
            slice0: alloc(),
            slice1: alloc(),
            noise000: 0.0,
            noise001: 0.0,
            noise100: 0.0,
            noise101: 0.0,
            noise010: 0.0,
            noise011: 0.0,
            noise110: 0.0,
            noise111: 0.0,
            value_xz00: 0.0,
            value_xz10: 0.0,
            value_xz01: 0.0,
            value_xz11: 0.0,
            value_z0: 0.0,
            value_z1: 0.0,
            value: 0.0,
        }
    }

    fn select_cell_yz(&mut self, cell_y: usize, cell_z: usize) {
        self.noise000 = self.slice0[cell_z][cell_y];
        self.noise001 = self.slice0[cell_z + 1][cell_y];
        self.noise100 = self.slice1[cell_z][cell_y];
        self.noise101 = self.slice1[cell_z + 1][cell_y];
        self.noise010 = self.slice0[cell_z][cell_y + 1];
        self.noise011 = self.slice0[cell_z + 1][cell_y + 1];
        self.noise110 = self.slice1[cell_z][cell_y + 1];
        self.noise111 = self.slice1[cell_z + 1][cell_y + 1];
    }

    fn update_for_y(&mut self, factor: f64) {
        self.value_xz00 = super::synth::lerp(factor, self.noise000, self.noise010);
        self.value_xz10 = super::synth::lerp(factor, self.noise100, self.noise110);
        self.value_xz01 = super::synth::lerp(factor, self.noise001, self.noise011);
        self.value_xz11 = super::synth::lerp(factor, self.noise101, self.noise111);
    }

    fn update_for_x(&mut self, factor: f64) {
        self.value_z0 = super::synth::lerp(factor, self.value_xz00, self.value_xz10);
        self.value_z1 = super::synth::lerp(factor, self.value_xz01, self.value_xz11);
    }

    fn update_for_z(&mut self, factor: f64) {
        self.value = super::synth::lerp(factor, self.value_z0, self.value_z1);
    }
}

/// `NoiseChunk.FlatCache` storage — quart-resolution 2D values, prefilled.
struct FlatState {
    values: Vec<f64>,
    size_xz: i32,
}

/// `NoiseChunk.Cache2D` storage.
struct Cache2DState {
    last_pos: i64,
    last_value: f64,
}

/// `NoiseChunk.CacheOnce` storage.
struct CacheOnceState {
    last_counter: i64,
    last_array_counter: i64,
    last_value: f64,
    last_array: Option<Vec<f64>>,
}

/// Which `ContextProvider` a `fillArray` runs under: the slice-filling
/// provider (one value per cell-Y) or the chunk itself (one per cell block).
#[derive(Clone, Copy)]
enum Provider {
    Slice,
    Cell,
}

impl Provider {
    /// `ContextProvider.forIndex` — positions the cursor for index `i`.
    fn for_index(self, st: &mut ChunkEvalState, i: usize) {
        match self {
            Provider::Slice => {
                st.cell_start_block_y = (i as i32 + st.cell_noise_min_y) * st.cell_height;
                st.interpolation_counter += 1;
                st.in_cell_y = 0;
                st.array_index = i;
            }
            Provider::Cell => {
                // NoiseChunk.forIndex
                let cell_index = i as i32;
                let z_in_cell = cell_index.rem_euclid(st.cell_width);
                let xy = cell_index.div_euclid(st.cell_width);
                let x_in_cell = xy.rem_euclid(st.cell_width);
                let y_in_cell = st.cell_height - 1 - xy.div_euclid(st.cell_width);
                st.in_cell_x = x_in_cell;
                st.in_cell_y = y_in_cell;
                st.in_cell_z = z_in_cell;
                st.array_index = i;
            }
        }
    }

    /// `ContextProvider.fillAllDirectly`.
    // Indexed loop mirrors the Java original: `i` also drives the cursor.
    #[allow(clippy::needless_range_loop)]
    fn fill_all_directly(self, out: &mut [f64], f: &Dfn, st: &mut ChunkEvalState) {
        match self {
            Provider::Slice => {
                for i in 0..out.len() {
                    st.cell_start_block_y = (i as i32 + st.cell_noise_min_y) * st.cell_height;
                    st.interpolation_counter += 1;
                    st.in_cell_y = 0;
                    st.array_index = i;
                    out[i] = f.compute(Ctx::Cursor, st);
                }
            }
            Provider::Cell => {
                st.array_index = 0;
                for y_in_cell in (0..st.cell_height).rev() {
                    st.in_cell_y = y_in_cell;
                    for x_in_cell in 0..st.cell_width {
                        st.in_cell_x = x_in_cell;
                        for z_in_cell in 0..st.cell_width {
                            st.in_cell_z = z_in_cell;
                            out[st.array_index] = f.compute(Ctx::Cursor, st);
                            st.array_index += 1;
                        }
                    }
                }
            }
        }
    }
}

impl Dfn {
    pub fn compute(&self, ctx: Ctx, st: &mut ChunkEvalState) -> f64 {
        match self {
            Dfn::Constant(v) => *v,
            Dfn::YClampedGradient { from_y, to_y, from_value, to_value } => clamped_map(
                ctx.block_y(st) as f64,
                *from_y as f64,
                *to_y as f64,
                *from_value,
                *to_value,
            ),
            Dfn::Noise { noise, xz_scale, y_scale } => noise.get_value(
                ctx.block_x(st) as f64 * xz_scale,
                ctx.block_y(st) as f64 * y_scale,
                ctx.block_z(st) as f64 * xz_scale,
            ),
            Dfn::ShiftA(n) => shift_noise(n, ctx.block_x(st) as f64, 0.0, ctx.block_z(st) as f64),
            Dfn::ShiftB(n) => shift_noise(n, ctx.block_z(st) as f64, ctx.block_x(st) as f64, 0.0),
            Dfn::Shift(n) => shift_noise(
                n,
                ctx.block_x(st) as f64,
                ctx.block_y(st) as f64,
                ctx.block_z(st) as f64,
            ),
            Dfn::ShiftedNoise { shift_x, shift_y, shift_z, xz_scale, y_scale, noise } => {
                let x = ctx.block_x(st) as f64 * xz_scale + shift_x.compute(ctx, st);
                let y = ctx.block_y(st) as f64 * y_scale + shift_y.compute(ctx, st);
                let z = ctx.block_z(st) as f64 * xz_scale + shift_z.compute(ctx, st);
                noise.get_value(x, y, z)
            }
            Dfn::BlendedNoise(n) => n.compute(ctx.block_x(st), ctx.block_y(st), ctx.block_z(st)),
            Dfn::BlendAlpha => 1.0,
            Dfn::BlendOffset => 0.0,
            Dfn::Beardifier => 0.0,
            Dfn::Marker { wrapped, .. } => wrapped.compute(ctx, st),
            Dfn::Clamp { input, min, max } => clamp(input.compute(ctx, st), *min, *max),
            Dfn::Mapped { kind, input, .. } => mapped_transform(*kind, input.compute(ctx, st)),
            Dfn::Ap2 { kind, argument1, argument2, .. } => {
                let v1 = argument1.compute(ctx, st);
                match kind {
                    Ap2Kind::Add => v1 + argument2.compute(ctx, st),
                    Ap2Kind::Mul => {
                        if v1 == 0.0 {
                            0.0
                        } else {
                            v1 * argument2.compute(ctx, st)
                        }
                    }
                    Ap2Kind::Min => {
                        if v1 < argument2.min_value() {
                            v1
                        } else {
                            v1.min(argument2.compute(ctx, st))
                        }
                    }
                    Ap2Kind::Max => {
                        if v1 > argument2.max_value() {
                            v1
                        } else {
                            v1.max(argument2.compute(ctx, st))
                        }
                    }
                }
            }
            Dfn::MulOrAdd { is_mul, input, argument, .. } => {
                let v = input.compute(ctx, st);
                if *is_mul { v * argument } else { v + argument }
            }
            Dfn::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                let v = input.compute(ctx, st);
                if v >= *min_inclusive && v < *max_exclusive {
                    when_in_range.compute(ctx, st)
                } else {
                    when_out_of_range.compute(ctx, st)
                }
            }
            Dfn::IntervalSelect { input, thresholds, functions } => {
                let v = input.compute(ctx, st);
                for (i, t) in thresholds.iter().enumerate() {
                    if v < *t {
                        return functions[i].compute(ctx, st);
                    }
                }
                functions.last().expect("interval_select functions").compute(ctx, st)
            }
            Dfn::Spline(s) => s.sample(ctx, st) as f64,
            Dfn::FindTopSurface { density, upper_bound, lower_bound, cell_height } => {
                let top_y = mth_floor(upper_bound.compute(ctx, st) / *cell_height as f64) * cell_height;
                if top_y <= *lower_bound {
                    return *lower_bound as f64;
                }
                let x = ctx.block_x(st);
                let z = ctx.block_z(st);
                let mut block_y = top_y;
                while block_y >= *lower_bound {
                    if density.compute(Ctx::Point { x, y: block_y, z }, st) > 0.0 {
                        return block_y as f64;
                    }
                    block_y -= cell_height;
                }
                *lower_bound as f64
            }
            Dfn::Interp { slot, filler } => match ctx {
                Ctx::Point { .. } => filler.compute(ctx, st),
                Ctx::Cursor => {
                    assert!(st.interpolating, "sampling interpolator outside the interpolation loop");
                    if st.filling_cell {
                        let s = &st.interp[*slot];
                        lerp3(
                            st.in_cell_x as f64 / st.cell_width as f64,
                            st.in_cell_y as f64 / st.cell_height as f64,
                            st.in_cell_z as f64 / st.cell_width as f64,
                            s.noise000,
                            s.noise100,
                            s.noise010,
                            s.noise110,
                            s.noise001,
                            s.noise101,
                            s.noise011,
                            s.noise111,
                        )
                    } else {
                        st.interp[*slot].value
                    }
                }
            },
            Dfn::FlatCacheNode { slot, filler } => {
                let ix = quart_from_block(ctx.block_x(st)) - st.first_noise_x;
                let iz = quart_from_block(ctx.block_z(st)) - st.first_noise_z;
                let size = st.flat[*slot].size_xz;
                if ix >= 0 && iz >= 0 && ix < size && iz < size {
                    st.flat[*slot].values[(ix + iz * size) as usize]
                } else {
                    filler.compute(ctx, st)
                }
            }
            Dfn::Cache2DNode { slot, filler } => {
                let pos = pack_2d(ctx.block_x(st), ctx.block_z(st));
                if st.c2d[*slot].last_pos == pos {
                    return st.c2d[*slot].last_value;
                }
                let v = filler.compute(ctx, st);
                let s = &mut st.c2d[*slot];
                s.last_pos = pos;
                s.last_value = v;
                v
            }
            Dfn::CacheOnceNode { slot, filler } => match ctx {
                Ctx::Point { .. } => filler.compute(ctx, st),
                Ctx::Cursor => {
                    {
                        let s = &st.once[*slot];
                        if let Some(a) = &s.last_array {
                            if s.last_array_counter == st.array_interpolation_counter {
                                return a[st.array_index];
                            }
                        }
                        if s.last_counter == st.interpolation_counter {
                            return s.last_value;
                        }
                    }
                    st.once[*slot].last_counter = st.interpolation_counter;
                    let v = filler.compute(ctx, st);
                    st.once[*slot].last_value = v;
                    v
                }
            },
            Dfn::CacheCellNode { slot, filler } => match ctx {
                Ctx::Point { .. } => filler.compute(ctx, st),
                Ctx::Cursor => {
                    assert!(st.interpolating, "sampling cell cache outside the interpolation loop");
                    let (x, y, z) = (st.in_cell_x, st.in_cell_y, st.in_cell_z);
                    if x >= 0 && y >= 0 && z >= 0 && x < st.cell_width && y < st.cell_height && z < st.cell_width
                    {
                        st.cell[*slot]
                            [(((st.cell_height - 1 - y) * st.cell_width + x) * st.cell_width + z) as usize]
                    } else {
                        filler.compute(ctx, st)
                    }
                }
            },
        }
    }

    /// `DensityFunction.fillArray` — the bulk-evaluation path. The per-type
    /// dispatch (which children fill whole arrays vs. get recomputed per
    /// index, and when `forIndex` runs) is replicated exactly: it drives the
    /// cursor counters that `CacheOnce` keys on.
    // Indexed loops mirror the Java originals: `i` positions the cursor via
    // `for_index` only on the branches vanilla does.
    #[allow(clippy::needless_range_loop)]
    fn fill_array(&self, out: &mut [f64], provider: Provider, st: &mut ChunkEvalState) {
        match self {
            Dfn::Constant(v) => out.fill(*v),
            Dfn::BlendAlpha => out.fill(1.0),
            Dfn::BlendOffset | Dfn::Beardifier => out.fill(0.0),
            Dfn::Marker { wrapped, .. } => wrapped.fill_array(out, provider, st),
            Dfn::Clamp { input, min, max } => {
                input.fill_array(out, provider, st);
                for v in out.iter_mut() {
                    *v = clamp(*v, *min, *max);
                }
            }
            Dfn::Mapped { kind, input, .. } => {
                input.fill_array(out, provider, st);
                for v in out.iter_mut() {
                    *v = mapped_transform(*kind, *v);
                }
            }
            Dfn::MulOrAdd { is_mul, input, argument, .. } => {
                input.fill_array(out, provider, st);
                for v in out.iter_mut() {
                    *v = if *is_mul { *v * argument } else { *v + argument };
                }
            }
            Dfn::Ap2 { kind, argument1, argument2, .. } => {
                argument1.fill_array(out, provider, st);
                match kind {
                    Ap2Kind::Add => {
                        let mut v2 = vec![0.0; out.len()];
                        argument2.fill_array(&mut v2, provider, st);
                        for (o, v) in out.iter_mut().zip(v2) {
                            *o += v;
                        }
                    }
                    Ap2Kind::Mul => {
                        for i in 0..out.len() {
                            let v = out[i];
                            out[i] = if v == 0.0 {
                                0.0
                            } else {
                                provider.for_index(st, i);
                                v * argument2.compute(Ctx::Cursor, st)
                            };
                        }
                    }
                    Ap2Kind::Min => {
                        let min = argument2.min_value();
                        for i in 0..out.len() {
                            let v = out[i];
                            out[i] = if v < min {
                                v
                            } else {
                                provider.for_index(st, i);
                                v.min(argument2.compute(Ctx::Cursor, st))
                            };
                        }
                    }
                    Ap2Kind::Max => {
                        let max = argument2.max_value();
                        for i in 0..out.len() {
                            let v = out[i];
                            out[i] = if v > max {
                                v
                            } else {
                                provider.for_index(st, i);
                                v.max(argument2.compute(Ctx::Cursor, st))
                            };
                        }
                    }
                }
            }
            Dfn::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                input.fill_array(out, provider, st);
                for i in 0..out.len() {
                    let v = out[i];
                    provider.for_index(st, i);
                    out[i] = if v >= *min_inclusive && v < *max_exclusive {
                        when_in_range.compute(Ctx::Cursor, st)
                    } else {
                        when_out_of_range.compute(Ctx::Cursor, st)
                    };
                }
            }
            Dfn::IntervalSelect { input, thresholds, functions } => {
                input.fill_array(out, provider, st);
                for i in 0..out.len() {
                    let v = out[i];
                    provider.for_index(st, i);
                    let mut chosen = functions.last().expect("interval_select functions");
                    for (k, t) in thresholds.iter().enumerate() {
                        if v < *t {
                            chosen = &functions[k];
                            break;
                        }
                    }
                    out[i] = chosen.compute(Ctx::Cursor, st);
                }
            }
            Dfn::Interp { filler, .. } => {
                if st.filling_cell {
                    provider.fill_all_directly(out, self, st);
                } else {
                    filler.fill_array(out, provider, st);
                }
            }
            Dfn::Cache2DNode { filler, .. } => filler.fill_array(out, provider, st),
            Dfn::CacheOnceNode { slot, filler } => {
                let valid = {
                    let s = &st.once[*slot];
                    s.last_array.is_some() && s.last_array_counter == st.array_interpolation_counter
                };
                if valid {
                    let a = st.once[*slot].last_array.take().expect("checked above");
                    out.copy_from_slice(&a[..out.len()]);
                    st.once[*slot].last_array = Some(a);
                } else {
                    filler.fill_array(out, provider, st);
                    let counter = st.array_interpolation_counter;
                    let s = &mut st.once[*slot];
                    match &mut s.last_array {
                        Some(a) if a.len() == out.len() => a.copy_from_slice(out),
                        _ => s.last_array = Some(out.to_vec()),
                    }
                    s.last_array_counter = counter;
                }
            }
            // Everything else is a SimpleFunction (or FlatCache/CacheAllInCell,
            // whose fillArray is fillAllDirectly too).
            _ => provider.fill_all_directly(out, self, st),
        }
    }
}

/// `DensityFunctions.ShiftNoise.compute`.
fn shift_noise(noise: &NormalNoise, x: f64, y: f64, z: f64) -> f64 {
    noise.get_value(x * 0.25, y * 0.25, z * 0.25) * 4.0
}

impl Spline {
    fn sample(&self, ctx: Ctx, st: &mut ChunkEvalState) -> f32 {
        match self {
            Spline::Constant(v) => *v,
            Spline::Multipoint { coordinate, locations, values, derivatives, .. } => {
                let input = coordinate.compute(ctx, st) as f32;
                let start = find_interval_start(locations, input);
                let last = locations.len() - 1;
                if start < 0 {
                    return linear_extend(input, locations, values[0].sample(ctx, st), derivatives, 0);
                }
                let start = start as usize;
                if start == last {
                    return linear_extend(
                        input,
                        locations,
                        values[last].sample(ctx, st),
                        derivatives,
                        last,
                    );
                }
                let x1 = locations[start];
                let x2 = locations[start + 1];
                let t = (input - x1) / (x2 - x1);
                let y1 = values[start].sample(ctx, st);
                let y2 = values[start + 1].sample(ctx, st);
                let a = derivatives[start] * (x2 - x1) - (y2 - y1);
                let b = -derivatives[start + 1] * (x2 - x1) + (y2 - y1);
                lerp_f32(t, y1, y2) + t * (1.0 - t) * lerp_f32(t, a, b)
            }
        }
    }
}

/// Evaluate an unwrapped (router-level) graph at a single position — vanilla's
/// `router.finalDensity().compute(new SinglePointContext(x, y, z))`.
pub fn compute_at(f: &Dfn, x: i32, y: i32, z: i32) -> f64 {
    let mut st = ChunkEvalState::standalone();
    f.compute(Ctx::Point { x, y, z }, &mut st)
}

// ---------------------------------------------------------------------------
// NoiseChunk — the per-chunk cell interpolator + fill driver
// ---------------------------------------------------------------------------

/// What `doFill` writes: air, the default block (stone), or a fluid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ParityBlock {
    Air = 0,
    Stone = 1,
    Water = 2,
    Lava = 3,
}

/// `NoiseChunk`: wraps the seeded router for one chunk (markers → concrete
/// caches, `blend_density` unwrapped, beardifier no-op) and drives the
/// 4×8-cell trilinear interpolation.
pub struct NoiseChunk {
    st: ChunkEvalState,
    /// Wrapped fillers behind each interpolator / cell-cache slot, in
    /// creation (fill) order.
    interp_fillers: Vec<Rc<Dfn>>,
    cell_fillers: Vec<Rc<Dfn>>,
    preliminary_surface: Rc<Dfn>,
    /// `cacheAllInCell(add(finalDensity, beardifier))`.
    full_noise_density: Rc<Dfn>,
    preliminary_cache: HashMap<i64, i32>,
    sea_level: i32,
    min_y: i32,
    height: i32,
}

impl NoiseChunk {
    pub fn for_chunk(rs: &RandomState, chunk_min_block_x: i32, chunk_min_block_z: i32) -> Self {
        let ns = rs.settings.noise;
        let cell_width = ns.cell_width();
        let cell_height = ns.cell_height();
        let cell_count_xz = 16 / cell_width;
        let st = ChunkEvalState {
            cell_width,
            cell_height,
            cell_count_xz,
            cell_count_y: ns.height.div_euclid(cell_height),
            cell_noise_min_y: ns.min_y.div_euclid(cell_height),
            first_cell_x: chunk_min_block_x.div_euclid(cell_width),
            first_cell_z: chunk_min_block_z.div_euclid(cell_width),
            first_noise_x: quart_from_block(chunk_min_block_x),
            first_noise_z: quart_from_block(chunk_min_block_z),
            noise_size_xz: quart_from_block(cell_count_xz * cell_width),
            ..ChunkEvalState::standalone()
        };
        let mut nc = Self {
            st,
            interp_fillers: Vec::new(),
            cell_fillers: Vec::new(),
            preliminary_surface: Rc::new(Dfn::Constant(0.0)),
            full_noise_density: Rc::new(Dfn::Constant(0.0)),
            preliminary_cache: HashMap::new(),
            sea_level: rs.settings.sea_level,
            min_y: ns.min_y,
            height: ns.height,
        };
        // Vanilla wraps the whole router; P2 needs only the two outputs the
        // fill path reads. Skipping the rest only skips their (transparent)
        // cache instances. P3 wires barrier/fluid/vein functions here.
        let mut memo: HashMap<*const Dfn, Rc<Dfn>> = HashMap::new();
        nc.preliminary_surface = nc.wrap(&rs.router.preliminary_surface_level, &mut memo);
        let final_density = nc.wrap(&rs.router.final_density, &mut memo);
        // fullNoiseDensity = cacheAllInCell(add(finalDensity, beardifier)) —
        // vanilla builds this post-wrap and re-runs mapAll, which dedups back
        // to exactly these two new nodes.
        let add = Rc::new(new_ap2(Ap2Kind::Add, final_density, Rc::new(Dfn::Beardifier)));
        nc.full_noise_density = nc.new_cell_cache(add);
        nc
    }

    /// `NoiseChunk.wrapNew` under `mapAll`: bottom-up rebuild with the memo
    /// map. Vanilla memoizes on structural equality; `Rc` pointer identity is
    /// coarser, which can only duplicate cache instances (value-neutral —
    /// every cache is value-transparent over a deterministic filler).
    fn wrap(&mut self, f: &Rc<Dfn>, memo: &mut HashMap<*const Dfn, Rc<Dfn>>) -> Rc<Dfn> {
        if let Some(w) = memo.get(&Rc::as_ptr(f)) {
            return w.clone();
        }
        let wrapped = self.wrap_new(f, memo);
        memo.insert(Rc::as_ptr(f), wrapped.clone());
        wrapped
    }

    fn wrap_new(&mut self, f: &Rc<Dfn>, memo: &mut HashMap<*const Dfn, Rc<Dfn>>) -> Rc<Dfn> {
        match &**f {
            Dfn::Marker { kind, wrapped } => {
                let inner = self.wrap(wrapped, memo);
                match kind {
                    MarkerKind::Interpolated => self.new_interpolator(inner),
                    MarkerKind::FlatCache => self.new_flat_cache(inner),
                    MarkerKind::Cache2D => {
                        let slot = self.st.c2d.len();
                        self.st.c2d.push(Cache2DState { last_pos: INVALID_POS_2D, last_value: 0.0 });
                        Rc::new(Dfn::Cache2DNode { slot, filler: inner })
                    }
                    MarkerKind::CacheOnce => {
                        let slot = self.st.once.len();
                        self.st.once.push(CacheOnceState {
                            last_counter: 0,
                            last_array_counter: 0,
                            last_value: 0.0,
                            last_array: None,
                        });
                        Rc::new(Dfn::CacheOnceNode { slot, filler: inner })
                    }
                    MarkerKind::CacheAllInCell => self.new_cell_cache(inner),
                    // Empty blender: blend_density unwraps to its input.
                    MarkerKind::BlendDensity => inner,
                }
            }
            Dfn::ShiftedNoise { shift_x, shift_y, shift_z, xz_scale, y_scale, noise } => {
                Rc::new(Dfn::ShiftedNoise {
                    shift_x: self.wrap(shift_x, memo),
                    shift_y: self.wrap(shift_y, memo),
                    shift_z: self.wrap(shift_z, memo),
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                    noise: noise.clone(),
                })
            }
            Dfn::Clamp { input, min, max } => {
                Rc::new(Dfn::Clamp { input: self.wrap(input, memo), min: *min, max: *max })
            }
            Dfn::Mapped { kind, input, .. } => Rc::new(new_mapped(*kind, self.wrap(input, memo))),
            Dfn::Ap2 { kind, argument1, argument2, .. } => Rc::new(new_ap2(
                *kind,
                self.wrap(argument1, memo),
                self.wrap(argument2, memo),
            )),
            Dfn::MulOrAdd { is_mul, input, argument, .. } => {
                // MulOrAdd.mapChildren recomputes bounds directly (not via
                // create) — same values, mirrored for faithfulness.
                let input = self.wrap(input, memo);
                let (in_min, in_max) = (input.min_value(), input.max_value());
                let (min, max) = if !*is_mul {
                    (in_min + argument, in_max + argument)
                } else if *argument >= 0.0 {
                    (in_min * argument, in_max * argument)
                } else {
                    (in_max * argument, in_min * argument)
                };
                Rc::new(Dfn::MulOrAdd { is_mul: *is_mul, input, argument: *argument, min, max })
            }
            Dfn::RangeChoice { input, min_inclusive, max_exclusive, when_in_range, when_out_of_range } => {
                Rc::new(Dfn::RangeChoice {
                    input: self.wrap(input, memo),
                    min_inclusive: *min_inclusive,
                    max_exclusive: *max_exclusive,
                    when_in_range: self.wrap(when_in_range, memo),
                    when_out_of_range: self.wrap(when_out_of_range, memo),
                })
            }
            Dfn::IntervalSelect { input, thresholds, functions } => Rc::new(Dfn::IntervalSelect {
                input: self.wrap(input, memo),
                thresholds: thresholds.clone(),
                functions: functions.iter().map(|f| self.wrap(f, memo)).collect(),
            }),
            Dfn::Spline(s) => Rc::new(Dfn::Spline(Rc::new(self.wrap_spline(s, memo)))),
            Dfn::FindTopSurface { density, upper_bound, lower_bound, cell_height } => {
                Rc::new(Dfn::FindTopSurface {
                    density: self.wrap(density, memo),
                    upper_bound: self.wrap(upper_bound, memo),
                    lower_bound: *lower_bound,
                    cell_height: *cell_height,
                })
            }
            // Leaves (constants, noises, blend alpha/offset, beardifier) pass
            // through unchanged, shared with the router graph.
            _ => f.clone(),
        }
    }

    /// `CubicSpline.mapCoordinates` — rebuild through the canonical
    /// constructor so min/max re-derive from the wrapped coordinate.
    fn wrap_spline(&mut self, s: &Spline, memo: &mut HashMap<*const Dfn, Rc<Dfn>>) -> Spline {
        match s {
            Spline::Constant(v) => Spline::Constant(*v),
            Spline::Multipoint { coordinate, locations, values, derivatives, .. } => {
                Spline::new_multipoint(
                    self.wrap(coordinate, memo),
                    locations.clone(),
                    values.iter().map(|v| self.wrap_spline(v, memo)).collect(),
                    derivatives.clone(),
                )
            }
        }
    }

    fn new_interpolator(&mut self, filler: Rc<Dfn>) -> Rc<Dfn> {
        let slot = self.st.interp.len();
        self.st.interp.push(InterpState::new(self.st.cell_count_y, self.st.cell_count_xz));
        self.interp_fillers.push(filler.clone());
        Rc::new(Dfn::Interp { slot, filler })
    }

    fn new_flat_cache(&mut self, filler: Rc<Dfn>) -> Rc<Dfn> {
        let size_xz = self.st.noise_size_xz + 1;
        let mut values = vec![0.0; (size_xz * size_xz) as usize];
        // Prefill at quart resolution, y = 0 (`new FlatCache(filler, true)`).
        for x in 0..=self.st.noise_size_xz {
            let block_x = quart_to_block(self.st.first_noise_x + x);
            for z in 0..=self.st.noise_size_xz {
                let block_z = quart_to_block(self.st.first_noise_z + z);
                values[(x + z * size_xz) as usize] =
                    filler.compute(Ctx::Point { x: block_x, y: 0, z: block_z }, &mut self.st);
            }
        }
        let slot = self.st.flat.len();
        self.st.flat.push(FlatState { values, size_xz });
        Rc::new(Dfn::FlatCacheNode { slot, filler })
    }

    fn new_cell_cache(&mut self, filler: Rc<Dfn>) -> Rc<Dfn> {
        let slot = self.st.cell.len();
        self.st.cell.push(vec![
            0.0;
            (self.st.cell_width * self.st.cell_width * self.st.cell_height)
                as usize
        ]);
        self.cell_fillers.push(filler.clone());
        Rc::new(Dfn::CacheCellNode { slot, filler })
    }

    // --- interpolation driver (the vanilla method set) ---

    fn fill_slice(&mut self, slice0: bool, cell_x: i32) {
        self.st.cell_start_block_x = cell_x * self.st.cell_width;
        self.st.in_cell_x = 0;
        for cell_z_index in 0..=(self.st.cell_count_xz as usize) {
            let cell_z = self.st.first_cell_z + cell_z_index as i32;
            self.st.cell_start_block_z = cell_z * self.st.cell_width;
            self.st.in_cell_z = 0;
            self.st.array_interpolation_counter += 1;
            for k in 0..self.interp_fillers.len() {
                let filler = self.interp_fillers[k].clone();
                let slices = if slice0 {
                    &mut self.st.interp[k].slice0
                } else {
                    &mut self.st.interp[k].slice1
                };
                let mut buf = std::mem::take(&mut slices[cell_z_index]);
                filler.fill_array(&mut buf, Provider::Slice, &mut self.st);
                let slices = if slice0 {
                    &mut self.st.interp[k].slice0
                } else {
                    &mut self.st.interp[k].slice1
                };
                slices[cell_z_index] = buf;
            }
        }
        self.st.array_interpolation_counter += 1;
    }

    pub fn initialize_for_first_cell_x(&mut self) {
        assert!(!self.st.interpolating, "starting interpolation twice");
        self.st.interpolating = true;
        self.st.interpolation_counter = 0;
        self.fill_slice(true, self.st.first_cell_x);
    }

    pub fn advance_cell_x(&mut self, cell_x_index: i32) {
        self.fill_slice(false, self.st.first_cell_x + cell_x_index + 1);
        self.st.cell_start_block_x = (self.st.first_cell_x + cell_x_index) * self.st.cell_width;
    }

    pub fn select_cell_yz(&mut self, cell_y_index: usize, cell_z_index: usize) {
        for s in &mut self.st.interp {
            s.select_cell_yz(cell_y_index, cell_z_index);
        }
        self.st.filling_cell = true;
        self.st.cell_start_block_y =
            (cell_y_index as i32 + self.st.cell_noise_min_y) * self.st.cell_height;
        self.st.cell_start_block_z =
            (self.st.first_cell_z + cell_z_index as i32) * self.st.cell_width;
        self.st.array_interpolation_counter += 1;
        for k in 0..self.cell_fillers.len() {
            let filler = self.cell_fillers[k].clone();
            let mut buf = std::mem::take(&mut self.st.cell[k]);
            filler.fill_array(&mut buf, Provider::Cell, &mut self.st);
            self.st.cell[k] = buf;
        }
        self.st.array_interpolation_counter += 1;
        self.st.filling_cell = false;
    }

    pub fn update_for_y(&mut self, pos_y: i32, factor: f64) {
        self.st.in_cell_y = pos_y - self.st.cell_start_block_y;
        for s in &mut self.st.interp {
            s.update_for_y(factor);
        }
    }

    pub fn update_for_x(&mut self, pos_x: i32, factor: f64) {
        self.st.in_cell_x = pos_x - self.st.cell_start_block_x;
        for s in &mut self.st.interp {
            s.update_for_x(factor);
        }
    }

    pub fn update_for_z(&mut self, pos_z: i32, factor: f64) {
        self.st.in_cell_z = pos_z - self.st.cell_start_block_z;
        self.st.interpolation_counter += 1;
        for s in &mut self.st.interp {
            s.update_for_z(factor);
        }
    }

    pub fn swap_slices(&mut self) {
        for s in &mut self.st.interp {
            std::mem::swap(&mut s.slice0, &mut s.slice1);
        }
    }

    pub fn stop_interpolation(&mut self) {
        assert!(self.st.interpolating, "stopping interpolation that never started");
        self.st.interpolating = false;
    }

    /// `getInterpolatedState` with the disabled aquifer
    /// (`Aquifer.createDisabled`): positive density → default block, else the
    /// global fluid picker. Real aquifers are P3.
    fn interpolated_state(&mut self) -> ParityBlock {
        let full = self.full_noise_density.clone();
        let density = full.compute(Ctx::Cursor, &mut self.st);
        if density > 0.0 {
            ParityBlock::Stone
        } else {
            self.fluid_at(self.st.cell_start_block_y + self.st.in_cell_y)
        }
    }

    /// `NoiseBasedChunkGenerator.createFluidPicker` + `FluidStatus.at`.
    fn fluid_at(&self, y: i32) -> ParityBlock {
        if y < (-54).min(self.sea_level) {
            if y < -54 { ParityBlock::Lava } else { ParityBlock::Air }
        } else if y < self.sea_level {
            ParityBlock::Water
        } else {
            ParityBlock::Air
        }
    }

    /// `NoiseChunk.preliminarySurfaceLevel` — quart-quantized, cached.
    pub fn preliminary_surface_level(&mut self, x: i32, z: i32) -> i32 {
        let qx = quart_to_block(quart_from_block(x));
        let qz = quart_to_block(quart_from_block(z));
        let key = pack_2d(qx, qz);
        if let Some(v) = self.preliminary_cache.get(&key) {
            return *v;
        }
        let f = self.preliminary_surface.clone();
        let v = mth_floor(f.compute(Ctx::Point { x: qx, y: 0, z: qz }, &mut self.st));
        self.preliminary_cache.insert(key, v);
        v
    }
}

// ---------------------------------------------------------------------------
// Chunk fill (doFill)
// ---------------------------------------------------------------------------

/// One filled chunk of parity terrain shape. Blocks index as
/// `((y - min_y) * 16 + z) * 16 + x`; heightmaps hold vanilla's
/// "first available" convention (highest matching block + 1, else `min_y`)
/// indexed `z * 16 + x`.
pub struct FilledChunk {
    pub min_y: i32,
    pub height: i32,
    pub blocks: Vec<ParityBlock>,
    pub ocean_floor_wg: [i32; 256],
    pub world_surface_wg: [i32; 256],
}

impl FilledChunk {
    pub fn block(&self, x: i32, y: i32, z: i32) -> ParityBlock {
        self.blocks[(((y - self.min_y) * 16 + z) * 16 + x) as usize]
    }
}

/// The P2 generator facade: a seeded overworld `RandomState` plus
/// `fillFromNoise`.
pub struct ParityGenerator {
    pub random_state: RandomState,
}

impl ParityGenerator {
    pub fn new_overworld(seed: i64) -> Self {
        let data = VanillaWorldgenData::load_overworld();
        Self { random_state: RandomState::new_overworld(&data, seed) }
    }

    /// `NoiseBasedChunkGenerator.doFill` — exact iteration order: advance X
    /// slices, then per Z column, cells top-down, `yInCell` descending, then
    /// `xInCell`, `zInCell`.
    pub fn fill_chunk(&self, chunk_x: i32, chunk_z: i32) -> FilledChunk {
        let min_block_x = chunk_x * 16;
        let min_block_z = chunk_z * 16;
        let mut nc = NoiseChunk::for_chunk(&self.random_state, min_block_x, min_block_z);
        let (cell_width, cell_height) = (nc.st.cell_width, nc.st.cell_height);
        let (cell_count_y, cell_min_y) = (nc.st.cell_count_y, nc.st.cell_noise_min_y);
        let (min_y, height) = (nc.min_y, nc.height);
        let mut out = FilledChunk {
            min_y,
            height,
            blocks: vec![ParityBlock::Air; (height as usize) * 256],
            ocean_floor_wg: [min_y; 256],
            world_surface_wg: [min_y; 256],
        };
        nc.initialize_for_first_cell_x();
        let cell_count_x = 16 / cell_width;
        let cell_count_z = 16 / cell_width;
        for cell_x_index in 0..cell_count_x {
            nc.advance_cell_x(cell_x_index);
            for cell_z_index in 0..cell_count_z {
                for cell_y_index in (0..cell_count_y).rev() {
                    nc.select_cell_yz(cell_y_index as usize, cell_z_index as usize);
                    for y_in_cell in (0..cell_height).rev() {
                        let pos_y = (cell_min_y + cell_y_index) * cell_height + y_in_cell;
                        nc.update_for_y(pos_y, y_in_cell as f64 / cell_height as f64);
                        for x_in_cell in 0..cell_width {
                            let pos_x = min_block_x + cell_x_index * cell_width + x_in_cell;
                            nc.update_for_x(pos_x, x_in_cell as f64 / cell_width as f64);
                            for z_in_cell in 0..cell_width {
                                let pos_z = min_block_z + cell_z_index * cell_width + z_in_cell;
                                nc.update_for_z(pos_z, z_in_cell as f64 / cell_width as f64);
                                let state = nc.interpolated_state();
                                if state == ParityBlock::Air {
                                    continue;
                                }
                                let (lx, lz) = (pos_x & 15, pos_z & 15);
                                out.blocks[(((pos_y - min_y) * 16 + lz) * 16 + lx) as usize] = state;
                                let column = (lz * 16 + lx) as usize;
                                // OCEAN_FLOOR_WG: blocks-motion (stone);
                                // WORLD_SURFACE_WG: any non-air.
                                if state == ParityBlock::Stone
                                    && out.ocean_floor_wg[column] < pos_y + 1
                                {
                                    out.ocean_floor_wg[column] = pos_y + 1;
                                }
                                if out.world_surface_wg[column] < pos_y + 1 {
                                    out.world_surface_wg[column] = pos_y + 1;
                                }
                            }
                        }
                    }
                }
            }
            nc.swap_slices();
        }
        nc.stop_interpolation();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overworld_router_builds() {
        let data = VanillaWorldgenData::load_overworld();
        assert_eq!(data.settings.noise.min_y, -64);
        assert_eq!(data.settings.noise.height, 384);
        assert_eq!(data.settings.noise.cell_width(), 4);
        assert_eq!(data.settings.noise.cell_height(), 8);
        assert_eq!(data.settings.sea_level, 63);
        assert!(!data.settings.legacy_random_source);
        let rs = RandomState::new_overworld(&data, 0);
        // Sanity: every router output evaluates and stays within its own
        // reported bounds at a handful of positions.
        let fields: [(&str, &Rc<Dfn>); 15] = [
            ("barrier", &rs.router.barrier),
            ("fluid_level_floodedness", &rs.router.fluid_level_floodedness),
            ("fluid_level_spread", &rs.router.fluid_level_spread),
            ("lava", &rs.router.lava),
            ("temperature", &rs.router.temperature),
            ("vegetation", &rs.router.vegetation),
            ("continents", &rs.router.continents),
            ("erosion", &rs.router.erosion),
            ("depth", &rs.router.depth),
            ("ridges", &rs.router.ridges),
            ("preliminary_surface_level", &rs.router.preliminary_surface_level),
            ("final_density", &rs.router.final_density),
            ("vein_toggle", &rs.router.vein_toggle),
            ("vein_ridged", &rs.router.vein_ridged),
            ("vein_gap", &rs.router.vein_gap),
        ];
        for (name, f) in fields {
            for (x, y, z) in [(0, 0, 0), (1000, 64, -2000), (-512, -60, 8888)] {
                let v = compute_at(f, x, y, z);
                assert!(
                    v >= f.min_value() && v <= f.max_value(),
                    "{name} at ({x},{y},{z}) = {v} outside [{}, {}]",
                    f.min_value(),
                    f.max_value()
                );
            }
        }
    }

    /// Bit-for-bit parity against the reference JVM (`VelaP2Harness` run on
    /// the real 26.2 server classes; aquifers/ore veins disabled — P3):
    /// all 15 router outputs at 8 positions × 3 seeds, plus full-chunk fill
    /// digests, sample columns, and preliminary surface levels for 2 chunks
    /// × 3 seeds.
    #[test]
    fn jvm_golden_parity() {
        let fixture = include_str!("testdata/p2_golden.txt");
        let mut generators: HashMap<i64, ParityGenerator> = HashMap::new();
        for line in fixture.lines() {
            let seed: i64 = line.split_whitespace().nth(1).expect("seed").parse().expect("seed");
            generators.entry(seed).or_insert_with(|| ParityGenerator::new_overworld(seed));
        }
        let generator = |seed: i64| -> &ParityGenerator { &generators[&seed] };
        let mut chunks: HashMap<(i64, i32, i32), FilledChunk> = HashMap::new();
        let mut checked = 0usize;
        for line in fixture.lines() {
            let mut parts = line.split_whitespace();
            let tag = parts.next().expect("tag");
            let seed: i64 = parts.next().expect("seed").parse().expect("seed");
            match tag {
                "router" => {
                    let name = parts.next().expect("name");
                    let x: i32 = parts.next().unwrap().parse().unwrap();
                    let y: i32 = parts.next().unwrap().parse().unwrap();
                    let z: i32 = parts.next().unwrap().parse().unwrap();
                    let bits = u64::from_str_radix(parts.next().expect("bits"), 16).unwrap();
                    let r = &generator(seed).random_state.router;
                    let f = match name {
                        "barrier" => &r.barrier,
                        "fluid_level_floodedness" => &r.fluid_level_floodedness,
                        "fluid_level_spread" => &r.fluid_level_spread,
                        "lava" => &r.lava,
                        "temperature" => &r.temperature,
                        "vegetation" => &r.vegetation,
                        "continents" => &r.continents,
                        "erosion" => &r.erosion,
                        "depth" => &r.depth,
                        "ridges" => &r.ridges,
                        "preliminary_surface_level" => &r.preliminary_surface_level,
                        "final_density" => &r.final_density,
                        "vein_toggle" => &r.vein_toggle,
                        "vein_ridged" => &r.vein_ridged,
                        "vein_gap" => &r.vein_gap,
                        other => panic!("unknown router field {other}"),
                    };
                    let v = compute_at(f, x, y, z);
                    assert_eq!(
                        v.to_bits(),
                        bits,
                        "router {name} seed {seed} at ({x},{y},{z}): got {v}, want {}",
                        f64::from_bits(bits)
                    );
                }
                "chunk" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let digest = u64::from_str_radix(parts.next().expect("digest"), 16).unwrap();
                    let chunk = generator(seed).fill_chunk(cx, cz);
                    let mut h = 0xcbf29ce484222325u64;
                    for b in &chunk.blocks {
                        h ^= *b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                    assert_eq!(h, digest, "chunk digest seed {seed} chunk ({cx},{cz})");
                    chunks.insert((seed, cx, cz), chunk);
                }
                "column" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let x: i32 = parts.next().unwrap().parse().unwrap();
                    let z: i32 = parts.next().unwrap().parse().unwrap();
                    let want = parts.next().expect("column blocks");
                    let chunk = &chunks[&(seed, cx, cz)];
                    let got: String = (0..chunk.height)
                        .map(|dy| match chunk.block(x, chunk.min_y + dy, z) {
                            ParityBlock::Air => '.',
                            ParityBlock::Stone => '_',
                            ParityBlock::Water => '~',
                            ParityBlock::Lava => 'L',
                        })
                        .collect();
                    assert_eq!(got, want, "column seed {seed} chunk ({cx},{cz}) at ({x},{z})");
                }
                "psl" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let want = parts.next().expect("levels");
                    let mut nc =
                        NoiseChunk::for_chunk(&generator(seed).random_state, cx * 16, cz * 16);
                    let got = (0..25)
                        .map(|i| {
                            nc.preliminary_surface_level(
                                cx * 16 + (i % 5) * 4,
                                cz * 16 + (i / 5) * 4,
                            )
                            .to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    assert_eq!(got, want, "psl seed {seed} chunk ({cx},{cz})");
                }
                other => panic!("unknown fixture tag {other}"),
            }
            checked += 1;
        }
        assert_eq!(checked, 390, "fixture line count");
    }

    #[test]
    fn fill_chunk_produces_plausible_terrain() {
        let generator = ParityGenerator::new_overworld(8000);
        let chunk = generator.fill_chunk(0, 0);
        // Bottom layer is solid essentially everywhere; the top of the world
        // is air; a surface exists between them.
        assert_eq!(chunk.block(0, -64, 0), ParityBlock::Stone);
        assert_eq!(chunk.block(8, 319, 8), ParityBlock::Air);
        for column in 0..256 {
            let ws = chunk.world_surface_wg[column];
            assert!(ws > -64 && ws < 320, "degenerate surface {ws}");
        }
    }
}
