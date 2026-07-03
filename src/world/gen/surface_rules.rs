//! Vanilla-parity surface rules (`SurfaceRules` + `SurfaceSystem`), P5 of
//! docs/WORLDGEN_PARITY.md.
//!
//! The rule tree is data-driven from `noise_settings/overworld.json`'s
//! `surface_rule` (the serialized `SurfaceRuleData.overworld()`), evaluated
//! per column over a noise-filled [`FilledChunk`]. Vanilla's lazily-memoized
//! `Context` state is all pure in position, so conditions here recompute
//! instead of memoizing — bit-identical output, no `lastUpdate` bookkeeping.
//! The stateful parts that *do* affect output are kept exact: the clay-band
//! array's RNG draws, the `BiomeManager` fuzzy zoom (sha256-obfuscated seed),
//! per-column heightmap updates feeding later columns' `steep` checks, and
//! the eroded-badlands / frozen-ocean extensions' draw order.

#![allow(dead_code)]

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::climate::{MultiNoiseBiomeSource, Sampler};
use super::density::{FilledChunk, NoiseChunk, ParityBlock, RandomState, VanillaWorldgenData};
use super::random::{PositionalRandomFactory, RandomSource};
use super::synth::{lerp2, NormalNoise, PerlinSimplexNoise};
use super::vanilla_jsons;

/// `Mth.floor`.
fn mth_floor(v: f64) -> i32 {
    v.floor() as i32
}

/// `Mth.map` (`lerp(inverseLerp(value, ...), ...)`).
fn mth_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    to_min + ((value - from_min) / (from_max - from_min)) * (to_max - to_min)
}

// ---------------------------------------------------------------------------
// Biome climate data + the static temperature noises
// ---------------------------------------------------------------------------

/// The `Biome.ClimateSettings` fields the surface rules read.
#[derive(Clone, Copy)]
struct BiomeClimate {
    temperature: f32,
    /// `temperature_modifier: "frozen"`.
    frozen: bool,
}

/// The three static `PerlinSimplexNoise` fields on `Biome`, seeded with the
/// fixed literals 1234 / 3456 / 2345 on the legacy LCG.
struct TemperatureNoises {
    temperature: PerlinSimplexNoise,
    frozen: PerlinSimplexNoise,
    biome_info: PerlinSimplexNoise,
}

fn temperature_noises() -> &'static TemperatureNoises {
    static NOISES: OnceLock<TemperatureNoises> = OnceLock::new();
    NOISES.get_or_init(|| TemperatureNoises {
        temperature: PerlinSimplexNoise::new(&mut RandomSource::legacy(1234), &[0]),
        frozen: PerlinSimplexNoise::new(&mut RandomSource::legacy(3456), &[-2, -1, 0]),
        biome_info: PerlinSimplexNoise::new(&mut RandomSource::legacy(2345), &[0]),
    })
}

/// `Biome.coldEnoughToSnow` for the FEATURES-stage `freeze_top_layer` — exposes
/// the exact temperature/frozen-modifier/height-adjust chain to `features.rs`.
pub fn cold_enough_to_snow(temperature: f32, frozen: bool, x: i32, y: i32, z: i32, sea_level: i32) -> bool {
    BiomeClimate { temperature, frozen }.cold_enough_to_snow(x, y, z, sea_level)
}

impl BiomeClimate {
    /// `Biome.TemperatureModifier.modifyTemperature`.
    fn modify_temperature(&self, x: i32, z: i32) -> f32 {
        if !self.frozen {
            return self.temperature;
        }
        let n = temperature_noises();
        let large = n.frozen.get_value_2d(x as f64 * 0.05, z as f64 * 0.05, false) * 7.0;
        let edge = n.biome_info.get_value_2d(x as f64 * 0.2, z as f64 * 0.2, false);
        if large + edge < 0.3 {
            let small = n.biome_info.get_value_2d(x as f64 * 0.09, z as f64 * 0.09, false);
            if small < 0.8 {
                return 0.2;
            }
        }
        self.temperature
    }

    /// `Biome.getHeightAdjustedTemperature` (float arithmetic preserved).
    fn height_adjusted_temperature(&self, x: i32, y: i32, z: i32, sea_level: i32) -> f32 {
        let adjusted = self.modify_temperature(x, z);
        let snow_level = sea_level + 17;
        if y > snow_level {
            let n = temperature_noises();
            let v = (n.temperature.get_value_2d(
                (x as f32 / 8.0) as f64,
                (z as f32 / 8.0) as f64,
                false,
            ) * 8.0) as f32;
            adjusted - (v + y as f32 - snow_level as f32) * 0.05 / 40.0
        } else {
            adjusted
        }
    }

    /// `Biome.coldEnoughToSnow`.
    fn cold_enough_to_snow(&self, x: i32, y: i32, z: i32, sea_level: i32) -> bool {
        !(self.height_adjusted_temperature(x, y, z, sea_level) >= 0.15)
    }

    /// `Biome.shouldMeltFrozenOceanIcebergSlightly`.
    fn should_melt_iceberg(&self, x: i32, y: i32, z: i32, sea_level: i32) -> bool {
        self.height_adjusted_temperature(x, y, z, sea_level) > 0.1
    }
}

// ---------------------------------------------------------------------------
// BiomeManager zoom
// ---------------------------------------------------------------------------

/// `BiomeManager.obfuscateSeed` — Guava `Hashing.sha256().hashLong(seed)
/// .asLong()`: sha256 over the little-endian seed bytes, first 8 digest
/// bytes read little-endian.
pub fn obfuscate_seed(seed: i64) -> i64 {
    let digest = Sha256::digest(seed.to_le_bytes());
    i64::from_le_bytes(digest[0..8].try_into().unwrap())
}

/// `LinearCongruentialGenerator.next`.
fn lcg_next(rval: i64, c: i64) -> i64 {
    rval.wrapping_mul(
        rval.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407),
    )
    .wrapping_add(c)
}

fn get_fiddle(rval: i64) -> f64 {
    let uniform = (rval >> 24).rem_euclid(1024) as f64 / 1024.0;
    (uniform - 0.5) * 0.9
}

fn get_fiddled_distance(seed: i64, x: i32, y: i32, z: i32, dx: f64, dy: f64, dz: f64) -> f64 {
    let (x, y, z) = (x as i64, y as i64, z as i64);
    let mut rval = seed;
    rval = lcg_next(rval, x);
    rval = lcg_next(rval, y);
    rval = lcg_next(rval, z);
    rval = lcg_next(rval, x);
    rval = lcg_next(rval, y);
    rval = lcg_next(rval, z);
    let fiddle_x = get_fiddle(rval);
    rval = lcg_next(rval, seed);
    let fiddle_y = get_fiddle(rval);
    rval = lcg_next(rval, seed);
    let fiddle_z = get_fiddle(rval);
    let sq = |v: f64| v * v;
    sq(dz + fiddle_z) + sq(dy + fiddle_y) + sq(dx + fiddle_x)
}

/// `BiomeManager.getBiome`'s fuzzy zoom: block position → the quart whose
/// fiddled distance wins.
pub fn zoomed_quart(biome_zoom_seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    let abs_x = x - 2;
    let abs_y = y - 2;
    let abs_z = z - 2;
    let parent_x = abs_x >> 2;
    let parent_y = abs_y >> 2;
    let parent_z = abs_z >> 2;
    let fract_x = (abs_x & 3) as f64 / 4.0;
    let fract_y = (abs_y & 3) as f64 / 4.0;
    let fract_z = (abs_z & 3) as f64 / 4.0;
    let mut min_i = 0;
    let mut min_distance = f64::INFINITY;
    for i in 0..8 {
        let x_even = (i & 4) == 0;
        let y_even = (i & 2) == 0;
        let z_even = (i & 1) == 0;
        let corner_x = if x_even { parent_x } else { parent_x + 1 };
        let corner_y = if y_even { parent_y } else { parent_y + 1 };
        let corner_z = if z_even { parent_z } else { parent_z + 1 };
        let dx = if x_even { fract_x } else { fract_x - 1.0 };
        let dy = if y_even { fract_y } else { fract_y - 1.0 };
        let dz = if z_even { fract_z } else { fract_z - 1.0 };
        let next = get_fiddled_distance(biome_zoom_seed, corner_x, corner_y, corner_z, dx, dy, dz);
        if min_distance > next {
            min_i = i;
            min_distance = next;
        }
    }
    (
        if (min_i & 4) == 0 { parent_x } else { parent_x + 1 },
        if (min_i & 2) == 0 { parent_y } else { parent_y + 1 },
        if (min_i & 1) == 0 { parent_z } else { parent_z + 1 },
    )
}

/// Baked per-quart biome ids for a generation neighborhood — the stand-in for
/// reading the proto chunks' biome palettes. Populated in the same section
/// visit order as `fillBiomesFromNoise`, chunk by chunk, so the stateful
/// RTree search evolves identically to the reference dump.
pub struct BakedBiomes {
    map: HashMap<(i32, i32, i32), u16>,
    quart_min_y: i32,
    quart_max_y: i32,
}

impl BakedBiomes {
    /// Bake the 3×3 chunk neighborhood around `(chunk_x, chunk_z)`, chunks in
    /// row-major `(dz, dx)` order.
    pub fn bake(
        source: &MultiNoiseBiomeSource,
        sampler: &Sampler,
        chunk_x: i32,
        chunk_z: i32,
        min_y: i32,
        height: i32,
    ) -> Self {
        let mut map = HashMap::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (cx, cz) = (chunk_x + dx, chunk_z + dz);
                let sections = source.fill_chunk_biomes(sampler, cx, cz, min_y, height);
                let quart_min_x = (cx * 16) >> 2;
                let quart_min_z = (cz * 16) >> 2;
                let min_section_y = min_y >> 4;
                for (section_index, section) in sections.iter().enumerate() {
                    let quart_min_y = (min_section_y + section_index as i32) << 2;
                    for y in 0..4 {
                        for z in 0..4 {
                            for x in 0..4 {
                                map.insert(
                                    (quart_min_x + x, quart_min_y + y, quart_min_z + z),
                                    section[((y * 4 + z) * 4 + x) as usize],
                                );
                            }
                        }
                    }
                }
            }
        }
        Self { map, quart_min_y: min_y >> 2, quart_max_y: (min_y + height - 1) >> 2 }
    }

    /// Assemble the baked view from *stored* per-section quart biomes (the
    /// layout `MultiNoiseBiomeSource::fill_chunk_biomes` produces) — the
    /// staged pipeline's path, where the SURFACE stage reads the biomes its
    /// dependency chunks computed at BIOMES rather than resampling.
    pub fn from_sections<'a>(
        chunks: impl IntoIterator<Item = ((i32, i32), &'a [[u16; 64]])>,
        min_y: i32,
        height: i32,
    ) -> Self {
        let mut map = HashMap::new();
        for ((cx, cz), sections) in chunks {
            let quart_min_x = (cx * 16) >> 2;
            let quart_min_z = (cz * 16) >> 2;
            let min_section_y = min_y >> 4;
            for (section_index, section) in sections.iter().enumerate() {
                let quart_min_y = (min_section_y + section_index as i32) << 2;
                for y in 0..4 {
                    for z in 0..4 {
                        for x in 0..4 {
                            map.insert(
                                (quart_min_x + x, quart_min_y + y, quart_min_z + z),
                                section[((y * 4 + z) * 4 + x) as usize],
                            );
                        }
                    }
                }
            }
        }
        Self { map, quart_min_y: min_y >> 2, quart_max_y: (min_y + height - 1) >> 2 }
    }

    /// `ChunkAccess.getNoiseBiome` — quart y clamped to the world range.
    fn get_noise_biome(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> u16 {
        let qy = quart_y.clamp(self.quart_min_y, self.quart_max_y);
        *self
            .map
            .get(&(quart_x, qy, quart_z))
            .unwrap_or_else(|| panic!("biome quart ({quart_x},{qy},{quart_z}) not baked"))
    }

    /// `BiomeManager.getBiome(BlockPos)` over the baked quarts.
    fn get_biome(&self, zoom_seed: i64, x: i32, y: i32, z: i32) -> u16 {
        let (qx, qy, qz) = zoomed_quart(zoom_seed, x, y, z);
        self.get_noise_biome(qx, qy, qz)
    }
}

// ---------------------------------------------------------------------------
// Rule tree (data-driven)
// ---------------------------------------------------------------------------

/// `VerticalAnchor`, pre-resolved against the generation heights at parse
/// time (they are constants for a given noise-settings).
fn resolve_anchor(v: &Value, min_y: i32, height: i32) -> i32 {
    if let Some(a) = v.get("absolute") {
        a.as_i64().expect("absolute") as i32
    } else if let Some(a) = v.get("above_bottom") {
        min_y + a.as_i64().expect("above_bottom") as i32
    } else if let Some(a) = v.get("below_top") {
        min_y + height - 1 - a.as_i64().expect("below_top") as i32
    } else {
        panic!("unknown vertical anchor {v}");
    }
}

enum Cond {
    Biome { biomes: Vec<u16> },
    NoiseThreshold { noise: Rc<NormalNoise>, min: f64, max: f64, is_3d: bool },
    VerticalGradient { true_at_and_below: i32, false_at_and_above: i32, factory: PositionalRandomFactory },
    YAbove { anchor: i32, surface_depth_multiplier: i32, add_stone_depth: bool },
    Water { offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool },
    Temperature,
    Steep,
    Hole,
    AbovePreliminarySurface,
    StoneDepth { offset: i32, add_surface_depth: bool, secondary_depth_range: i32, ceiling: bool },
    Not(Box<Cond>),
}

enum Rule {
    Block(ParityBlock),
    Sequence(Vec<Rule>),
    Test { condition: Cond, then_run: Box<Rule> },
    Bandlands,
}

/// Shared bits the parser needs.
struct RuleParser<'a> {
    data: &'a VanillaWorldgenData,
    rs: &'a RandomState,
    source: &'a MultiNoiseBiomeSource,
    noise_cache: HashMap<String, Rc<NormalNoise>>,
    min_y: i32,
    height: i32,
}

impl RuleParser<'_> {
    /// `RandomState.getOrCreateNoise` — seeded `fromHashOf(noise id)` off the
    /// base factory, so creation order is irrelevant.
    fn noise(&mut self, id: &str) -> Rc<NormalNoise> {
        if let Some(n) = self.noise_cache.get(id) {
            return n.clone();
        }
        let params = self.data.noise_parameters(id);
        let noise = Rc::new(NormalNoise::create(&mut self.rs.random.from_hash_of(id), params));
        self.noise_cache.insert(id.to_string(), noise.clone());
        noise
    }

    fn condition(&mut self, v: &Value) -> Cond {
        let ty = v["type"].as_str().expect("condition type");
        match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "biome" => {
                let list = match &v["biome_is"] {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => {
                        a.iter().map(|e| e.as_str().expect("biome id").to_owned()).collect()
                    }
                    other => panic!("bad biome_is {other}"),
                };
                let biomes = list
                    .iter()
                    .filter_map(|name| self.source.biome_index(name))
                    .collect();
                Cond::Biome { biomes }
            }
            "noise_threshold" => Cond::NoiseThreshold {
                noise: self.noise(v["noise"].as_str().expect("noise id")),
                min: v["min_threshold"].as_f64().expect("min_threshold"),
                max: v["max_threshold"].as_f64().expect("max_threshold"),
                is_3d: v.get("is_3d").and_then(Value::as_bool).unwrap_or(false),
            },
            "vertical_gradient" => Cond::VerticalGradient {
                true_at_and_below: resolve_anchor(&v["true_at_and_below"], self.min_y, self.height),
                false_at_and_above: resolve_anchor(&v["false_at_and_above"], self.min_y, self.height),
                // `RandomState.getOrCreateRandomFactory(random_name)`.
                factory: self
                    .rs
                    .random
                    .from_hash_of(v["random_name"].as_str().expect("random_name"))
                    .fork_positional(),
            },
            "y_above" => Cond::YAbove {
                anchor: resolve_anchor(&v["anchor"], self.min_y, self.height),
                surface_depth_multiplier: v["surface_depth_multiplier"].as_i64().unwrap() as i32,
                add_stone_depth: v["add_stone_depth"].as_bool().unwrap(),
            },
            "water" => Cond::Water {
                offset: v["offset"].as_i64().unwrap() as i32,
                surface_depth_multiplier: v["surface_depth_multiplier"].as_i64().unwrap() as i32,
                add_stone_depth: v["add_stone_depth"].as_bool().unwrap(),
            },
            "temperature" => Cond::Temperature,
            "steep" => Cond::Steep,
            "hole" => Cond::Hole,
            "above_preliminary_surface" => Cond::AbovePreliminarySurface,
            "stone_depth" => Cond::StoneDepth {
                offset: v["offset"].as_i64().unwrap() as i32,
                add_surface_depth: v["add_surface_depth"].as_bool().unwrap(),
                secondary_depth_range: v["secondary_depth_range"].as_i64().unwrap() as i32,
                ceiling: v["surface_type"].as_str().expect("surface_type") == "ceiling",
            },
            "not" => Cond::Not(Box::new(self.condition(&v["invert"]))),
            other => panic!("unknown surface condition {other}"),
        }
    }

    fn rule(&mut self, v: &Value) -> Rule {
        let ty = v["type"].as_str().expect("rule type");
        match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "block" => {
                let name = v["result_state"]["Name"].as_str().expect("result_state Name");
                Rule::Block(
                    ParityBlock::from_name(name)
                        .unwrap_or_else(|| panic!("unmapped surface block {name}")),
                )
            }
            "sequence" => Rule::Sequence(
                v["sequence"].as_array().expect("sequence").iter().map(|r| self.rule(r)).collect(),
            ),
            "condition" => Rule::Test {
                condition: self.condition(&v["if_true"]),
                then_run: Box::new(self.rule(&v["then_run"])),
            },
            "bandlands" => Rule::Bandlands,
            other => panic!("unknown surface rule {other}"),
        }
    }
}

// ---------------------------------------------------------------------------
// SurfaceSystem
// ---------------------------------------------------------------------------

/// `SurfaceSystem` + the compiled overworld rule tree + the biome tables.
pub struct SurfaceSystem {
    rule: Rule,
    default_block: ParityBlock,
    sea_level: i32,
    min_y: i32,
    height: i32,
    biome_zoom_seed: i64,
    /// `Biome.ClimateSettings` per biome id of the multi-noise source.
    biome_climate: Vec<BiomeClimate>,
    /// Special-cased biome ids (may be absent from the source in principle).
    eroded_badlands: Option<u16>,
    frozen_ocean: Option<u16>,
    deep_frozen_ocean: Option<u16>,
    clay_bands: [ParityBlock; 192],
    clay_bands_offset_noise: Rc<NormalNoise>,
    badlands_pillar_noise: Rc<NormalNoise>,
    badlands_pillar_roof_noise: Rc<NormalNoise>,
    badlands_surface_noise: Rc<NormalNoise>,
    iceberg_pillar_noise: Rc<NormalNoise>,
    iceberg_pillar_roof_noise: Rc<NormalNoise>,
    iceberg_surface_noise: Rc<NormalNoise>,
    surface_noise: Rc<NormalNoise>,
    surface_secondary_noise: Rc<NormalNoise>,
    noise_random: PositionalRandomFactory,
}

impl SurfaceSystem {
    pub fn new(
        data: &VanillaWorldgenData,
        rs: &RandomState,
        source: &MultiNoiseBiomeSource,
        seed: i64,
    ) -> Self {
        let min_y = rs.settings.noise.min_y;
        let height = rs.settings.noise.height;
        let mut parser = RuleParser {
            data,
            rs,
            source,
            noise_cache: HashMap::new(),
            min_y,
            height,
        };
        let rule = parser.rule(&data.settings.surface_rule);
        let mut noise = |id: &str| parser.noise(id);
        let clay_bands_offset_noise = noise("minecraft:clay_bands_offset");
        let surface_noise = noise("minecraft:surface");
        let surface_secondary_noise = noise("minecraft:surface_secondary");
        let badlands_pillar_noise = noise("minecraft:badlands_pillar");
        let badlands_pillar_roof_noise = noise("minecraft:badlands_pillar_roof");
        let badlands_surface_noise = noise("minecraft:badlands_surface");
        let iceberg_pillar_noise = noise("minecraft:iceberg_pillar");
        let iceberg_pillar_roof_noise = noise("minecraft:iceberg_pillar_roof");
        let iceberg_surface_noise = noise("minecraft:iceberg_surface");
        let noise_random = rs.random;
        let clay_bands =
            generate_bands(&mut noise_random.from_hash_of("minecraft:clay_bands"));

        // Biome climate settings, aligned with the source's biome id table.
        let mut by_name: HashMap<String, BiomeClimate> = HashMap::new();
        for &(short, json) in vanilla_jsons::BIOMES {
            let v: Value = serde_json::from_str(json).expect("bad biome JSON");
            by_name.insert(
                format!("minecraft:{short}"),
                BiomeClimate {
                    temperature: v["temperature"].as_f64().expect("temperature") as f32,
                    frozen: v
                        .get("temperature_modifier")
                        .and_then(Value::as_str)
                        .is_some_and(|m| m == "frozen"),
                },
            );
        }
        let biome_climate = (0..source.biome_count())
            .map(|id| {
                let name = source.biome_name(id as u16);
                *by_name
                    .get(name)
                    .unwrap_or_else(|| panic!("no vendored biome JSON for {name}"))
            })
            .collect();

        Self {
            rule,
            default_block: ParityBlock::Stone,
            sea_level: rs.settings.sea_level,
            min_y,
            height,
            biome_zoom_seed: obfuscate_seed(seed),
            biome_climate,
            eroded_badlands: source.biome_index("minecraft:eroded_badlands"),
            frozen_ocean: source.biome_index("minecraft:frozen_ocean"),
            deep_frozen_ocean: source.biome_index("minecraft:deep_frozen_ocean"),
            clay_bands,
            clay_bands_offset_noise,
            badlands_pillar_noise,
            badlands_pillar_roof_noise,
            badlands_surface_noise,
            iceberg_pillar_noise,
            iceberg_pillar_roof_noise,
            iceberg_surface_noise,
            surface_noise,
            surface_secondary_noise,
            noise_random,
        }
    }

    /// `SurfaceSystem.getSurfaceDepth`.
    fn get_surface_depth(&self, block_x: i32, block_z: i32) -> i32 {
        let noise_value = self.surface_noise.get_value(block_x as f64, 0.0, block_z as f64);
        let jitter = self.noise_random.at(block_x, 0, block_z).next_double() * 0.25;
        (noise_value * 2.75 + 3.0 + jitter) as i32
    }

    fn get_surface_secondary(&self, block_x: i32, block_z: i32) -> f64 {
        self.surface_secondary_noise.get_value(block_x as f64, 0.0, block_z as f64)
    }

    /// `SurfaceSystem.getBand`.
    fn get_band(&self, world_x: i32, y: i32, world_z: i32) -> ParityBlock {
        let v = self.clay_bands_offset_noise.get_value(world_x as f64, 0.0, world_z as f64);
        // `Math.round(double)` is floor(x + 0.5).
        let offset = (v * 4.0 + 0.5).floor() as i32;
        let len = self.clay_bands.len() as i32;
        self.clay_bands[((y + offset + len) % len) as usize]
    }

    /// `SurfaceSystem.buildSurface` over one noise-filled chunk (the
    /// `useLegacyRandom = false` path; `possibleBiomes = null`).
    pub fn build_surface(
        &self,
        chunk: &mut FilledChunk,
        noise_chunk: &mut NoiseChunk,
        biomes: &BakedBiomes,
        chunk_x: i32,
        chunk_z: i32,
    ) {
        let min_block_x = chunk_x * 16;
        let min_block_z = chunk_z * 16;
        for x in 0..16 {
            for z in 0..16 {
                let block_x = min_block_x + x;
                let block_z = min_block_z + z;
                let column = (z * 16 + x) as usize;
                let starting_height = chunk.world_surface_wg[column];
                let surface_biome =
                    biomes.get_biome(self.biome_zoom_seed, block_x, starting_height, block_z);
                if Some(surface_biome) == self.eroded_badlands {
                    self.eroded_badlands_extension(chunk, block_x, block_z, starting_height);
                }

                let height = chunk.world_surface_wg[column];
                let mut ctx = ColumnCtx {
                    system: self,
                    chunk,
                    noise_chunk,
                    biomes,
                    block_x,
                    block_z,
                    surface_depth: self.get_surface_depth(block_x, block_z),
                    block_y: 0,
                    water_height: i32::MIN,
                    stone_depth_above: 0,
                    stone_depth_below: 0,
                };
                let mut stone_above_depth = 0;
                let mut water_height = i32::MIN;
                let mut next_ceiling_stone_y = i32::MAX;
                let end_y = ctx.chunk.min_y;

                for y in (end_y..=height).rev() {
                    let old = block_at(ctx.chunk, x, y, z);
                    if old.is_air() {
                        stone_above_depth = 0;
                        water_height = i32::MIN;
                    } else if old.is_fluid() {
                        if water_height == i32::MIN {
                            water_height = y + 1;
                        }
                    } else {
                        if next_ceiling_stone_y >= y {
                            // `DimensionType.WAY_BELOW_MIN_Y`.
                            next_ceiling_stone_y = -2032 << 4;
                            for lookahead_y in (end_y - 1..=y - 1).rev() {
                                let next_state = block_at(ctx.chunk, x, lookahead_y, z);
                                if !is_stone(next_state) {
                                    next_ceiling_stone_y = lookahead_y + 1;
                                    break;
                                }
                            }
                        }
                        stone_above_depth += 1;
                        let stone_below_depth = y - next_ceiling_stone_y + 1;
                        ctx.block_y = y;
                        ctx.water_height = water_height;
                        ctx.stone_depth_above = stone_above_depth;
                        ctx.stone_depth_below = stone_below_depth;
                        if old == self.default_block {
                            if let Some(state) = try_apply(&self.rule, &mut ctx) {
                                set_block(ctx.chunk, x, y, z, state);
                            }
                        }
                    }
                }

                let min_surface_level = ctx.min_surface_level();
                drop(ctx);
                if Some(surface_biome) == self.frozen_ocean
                    || Some(surface_biome) == self.deep_frozen_ocean
                {
                    self.frozen_ocean_extension(
                        chunk,
                        min_surface_level,
                        surface_biome,
                        block_x,
                        block_z,
                        starting_height,
                    );
                }
            }
        }
    }

    /// `SurfaceSystem.topMaterial` — evaluate the surface rule at one arbitrary
    /// position and return the resulting block, or `None`. Used by carvers to
    /// convert the dirt exposed directly beneath a carved grass/mycelium column
    /// into the biome's top material (`WorldCarver.carveBlock`). Matches vanilla:
    /// a fresh `SurfaceRules.Context` with `stoneDepthAbove = stoneDepthBelow = 1`
    /// and `waterHeight = underFluid ? y + 1 : Integer.MIN_VALUE`.
    pub fn top_material(
        &self,
        chunk: &mut FilledChunk,
        noise_chunk: &mut NoiseChunk,
        biomes: &BakedBiomes,
        block_x: i32,
        block_y: i32,
        block_z: i32,
        under_fluid: bool,
    ) -> Option<ParityBlock> {
        let mut ctx = ColumnCtx {
            system: self,
            chunk,
            noise_chunk,
            biomes,
            block_x,
            block_z,
            surface_depth: self.get_surface_depth(block_x, block_z),
            block_y,
            water_height: if under_fluid { block_y + 1 } else { i32::MIN },
            stone_depth_above: 1,
            stone_depth_below: 1,
        };
        try_apply(&self.rule, &mut ctx)
    }

    /// `SurfaceSystem.erodedBadlandsExtension`.
    fn eroded_badlands_extension(
        &self,
        chunk: &mut FilledChunk,
        block_x: i32,
        block_z: i32,
        height: i32,
    ) {
        let (x, z) = (block_x & 15, block_z & 15);
        let pillar = f64::min(
            (self.badlands_surface_noise.get_value(block_x as f64, 0.0, block_z as f64) * 8.25)
                .abs(),
            self.badlands_pillar_noise.get_value(block_x as f64 * 0.2, 0.0, block_z as f64 * 0.2)
                * 15.0,
        );
        if pillar <= 0.0 {
            return;
        }
        let pillar_roof = (self
            .badlands_pillar_roof_noise
            .get_value(block_x as f64 * 0.75, 0.0, block_z as f64 * 0.75)
            * 1.5)
            .abs();
        let extension_top =
            64.0 + f64::min(pillar * pillar * 2.5, (pillar_roof * 50.0).ceil() + 24.0);
        let start_y = mth_floor(extension_top);
        if height > start_y {
            return;
        }
        for y in (chunk.min_y..=start_y).rev() {
            let old = block_at(chunk, x, y, z);
            if old == self.default_block {
                break;
            }
            if old == ParityBlock::Water {
                return;
            }
        }
        let mut y = start_y;
        while y >= chunk.min_y && block_at(chunk, x, y, z).is_air() {
            set_block(chunk, x, y, z, self.default_block);
            y -= 1;
        }
    }

    /// `SurfaceSystem.frozenOceanExtension`.
    fn frozen_ocean_extension(
        &self,
        chunk: &mut FilledChunk,
        min_surface_level: i32,
        surface_biome: u16,
        block_x: i32,
        block_z: i32,
        height: i32,
    ) {
        let (x, z) = (block_x & 15, block_z & 15);
        let iceberg = f64::min(
            (self.iceberg_surface_noise.get_value(block_x as f64, 0.0, block_z as f64) * 8.25)
                .abs(),
            self.iceberg_pillar_noise.get_value(block_x as f64 * 1.28, 0.0, block_z as f64 * 1.28)
                * 15.0,
        );
        if iceberg <= 1.8 {
            return;
        }
        let iceberg_roof = (self
            .iceberg_pillar_roof_noise
            .get_value(block_x as f64 * 1.17, 0.0, block_z as f64 * 1.17)
            * 1.5)
            .abs();
        let mut top = f64::min(iceberg * iceberg * 1.2, (iceberg_roof * 40.0).ceil() + 14.0);
        let climate = self.biome_climate[surface_biome as usize];
        if climate.should_melt_iceberg(block_x, self.sea_level, block_z, self.sea_level) {
            top -= 2.0;
        }
        let extension_bottom;
        if top > 2.0 {
            extension_bottom = self.sea_level as f64 - top - 7.0;
            top += self.sea_level as f64;
        } else {
            top = 0.0;
            extension_bottom = 0.0;
        }
        let extension_top = top;
        let mut random = self.noise_random.at(block_x, 0, block_z);
        let max_snow_depth = 2 + random.next_int_bounded(4);
        let min_snow_height = self.sea_level + 18 + random.next_int_bounded(10);
        let mut snow_depth = 0;
        for y in (min_surface_level..=i32::max(height, extension_top as i32 + 1)).rev() {
            let state = block_at(chunk, x, y, z);
            let place = (state.is_air()
                && y < extension_top as i32
                && random.next_double() > 0.01)
                || (state == ParityBlock::Water
                    && y > extension_bottom as i32
                    && y < self.sea_level
                    && extension_bottom != 0.0
                    && random.next_double() > 0.15);
            if place {
                if snow_depth <= max_snow_depth && y > min_snow_height {
                    set_block(chunk, x, y, z, ParityBlock::SnowBlock);
                    snow_depth += 1;
                } else {
                    set_block(chunk, x, y, z, ParityBlock::PackedIce);
                }
            }
        }
    }
}

/// `SurfaceSystem.generateBands`.
fn generate_bands(random: &mut RandomSource) -> [ParityBlock; 192] {
    use ParityBlock::*;
    let mut bands = [Terracotta; 192];
    let len = bands.len() as i32;
    let mut i = 0i32;
    while i < len {
        i += random.next_int_bounded(5) + 1;
        if i < len {
            bands[i as usize] = OrangeTerracotta;
        }
        i += 1;
    }
    make_bands(random, &mut bands, 1, YellowTerracotta);
    make_bands(random, &mut bands, 2, BrownTerracotta);
    make_bands(random, &mut bands, 1, RedTerracotta);
    let white_band_count = random.next_int_inclusive(9, 15);
    let mut count = 0;
    let mut start = 0i32;
    while count < white_band_count && start < len {
        bands[start as usize] = WhiteTerracotta;
        if start - 1 > 0 && random.next_boolean() {
            bands[(start - 1) as usize] = LightGrayTerracotta;
        }
        if start + 1 < len && random.next_boolean() {
            bands[(start + 1) as usize] = LightGrayTerracotta;
        }
        count += 1;
        start += random.next_int_bounded(16) + 4;
    }
    bands
}

fn make_bands(
    random: &mut RandomSource,
    bands: &mut [ParityBlock; 192],
    base_width: i32,
    state: ParityBlock,
) {
    let band_count = random.next_int_inclusive(6, 15);
    for _ in 0..band_count {
        let width = base_width + random.next_int_bounded(3);
        let start = random.next_int_bounded(bands.len() as i32);
        let mut p = 0;
        while start + p < bands.len() as i32 && p < width {
            bands[(start + p) as usize] = state;
            p += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Per-column evaluation context
// ---------------------------------------------------------------------------

/// `SurfaceRules.Context`, minus the lazy memoization (see module docs).
struct ColumnCtx<'a> {
    system: &'a SurfaceSystem,
    chunk: &'a mut FilledChunk,
    noise_chunk: &'a mut NoiseChunk,
    biomes: &'a BakedBiomes,
    block_x: i32,
    block_z: i32,
    surface_depth: i32,
    block_y: i32,
    water_height: i32,
    stone_depth_above: i32,
    stone_depth_below: i32,
}

impl ColumnCtx<'_> {
    fn get_biome(&self) -> u16 {
        self.biomes.get_biome(
            self.system.biome_zoom_seed,
            self.block_x,
            self.block_y,
            self.block_z,
        )
    }

    /// `Context.getMinSurfaceLevel` — bilinear over the 16-block surface-cell
    /// corners' preliminary surface levels.
    fn min_surface_level(&mut self) -> i32 {
        let cell_x = self.block_x >> 4;
        let cell_z = self.block_z >> 4;
        let c00 = self.noise_chunk.preliminary_surface_level(cell_x << 4, cell_z << 4);
        let c10 = self.noise_chunk.preliminary_surface_level((cell_x + 1) << 4, cell_z << 4);
        let c01 = self.noise_chunk.preliminary_surface_level(cell_x << 4, (cell_z + 1) << 4);
        let c11 = self.noise_chunk.preliminary_surface_level((cell_x + 1) << 4, (cell_z + 1) << 4);
        let level = mth_floor(lerp2(
            ((self.block_x & 15) as f32 / 16.0) as f64,
            ((self.block_z & 15) as f32 / 16.0) as f64,
            c00 as f64,
            c10 as f64,
            c01 as f64,
            c11 as f64,
        ));
        level + self.surface_depth - 8
    }
}

fn block_at(chunk: &FilledChunk, x: i32, y: i32, z: i32) -> ParityBlock {
    if y < chunk.min_y || y >= chunk.min_y + chunk.height {
        return ParityBlock::Air;
    }
    chunk.block(x, y, z)
}

/// `Heightmap.Types.WORLD_SURFACE_WG` predicate.
fn hm_not_air(state: ParityBlock) -> bool {
    !state.is_air()
}

/// `Heightmap.Types.OCEAN_FLOOR_WG` predicate (`blocksMotion`): solids except
/// powder snow; fluids and air excluded.
fn hm_blocks_motion(state: ParityBlock) -> bool {
    !state.is_air() && !state.is_fluid() && state != ParityBlock::PowderSnow
}

/// `Heightmap.update` for one worldgen heightmap, evaluated against the
/// already-written chunk (vanilla writes the section first, then updates).
/// `first` is the stored "first available" value for the column.
fn heightmap_update(
    chunk: &FilledChunk,
    first: i32,
    x: i32,
    y: i32,
    z: i32,
    state: ParityBlock,
    is_opaque: fn(ParityBlock) -> bool,
) -> i32 {
    if y <= first - 2 {
        return first;
    }
    if is_opaque(state) {
        if y >= first {
            return y + 1;
        }
    } else if first - 1 == y {
        for scan_y in (chunk.min_y..=y - 1).rev() {
            if is_opaque(block_at(chunk, x, scan_y, z)) {
                return scan_y + 1;
            }
        }
        return chunk.min_y;
    }
    first
}

/// `BlockColumn.setBlock` through the proto chunk: writes the state and
/// updates both worldgen heightmaps (`ProtoChunk.setBlockState`).
fn set_block(chunk: &mut FilledChunk, x: i32, y: i32, z: i32, state: ParityBlock) {
    if y < chunk.min_y || y >= chunk.min_y + chunk.height {
        return;
    }
    let column = (z * 16 + x) as usize;
    chunk.blocks[(((y - chunk.min_y) * 16 + z) * 16 + x) as usize] = state;
    let ws = heightmap_update(chunk, chunk.world_surface_wg[column], x, y, z, state, hm_not_air);
    chunk.world_surface_wg[column] = ws;
    let of =
        heightmap_update(chunk, chunk.ocean_floor_wg[column], x, y, z, state, hm_blocks_motion);
    chunk.ocean_floor_wg[column] = of;
}

/// `SurfaceSystem.isStone`.
fn is_stone(state: ParityBlock) -> bool {
    !state.is_air() && !state.is_fluid()
}

fn test_condition(cond: &Cond, ctx: &mut ColumnCtx) -> bool {
    match cond {
        Cond::Biome { biomes } => biomes.contains(&ctx.get_biome()),
        Cond::NoiseThreshold { noise, min, max, is_3d } => {
            let value = if *is_3d {
                noise.get_value(ctx.block_x as f64, ctx.block_y as f64, ctx.block_z as f64)
            } else {
                noise.get_value(ctx.block_x as f64, 0.0, ctx.block_z as f64)
            };
            value >= *min && value <= *max
        }
        Cond::VerticalGradient { true_at_and_below, false_at_and_above, factory } => {
            let y = ctx.block_y;
            if y <= *true_at_and_below {
                return true;
            }
            if y >= *false_at_and_above {
                return false;
            }
            let probability = mth_map(
                y as f64,
                *true_at_and_below as f64,
                *false_at_and_above as f64,
                1.0,
                0.0,
            );
            let mut random = factory.at(ctx.block_x, y, ctx.block_z);
            (random.next_float() as f64) < probability
        }
        Cond::YAbove { anchor, surface_depth_multiplier, add_stone_depth } => {
            ctx.block_y + if *add_stone_depth { ctx.stone_depth_above } else { 0 }
                >= *anchor + ctx.surface_depth * *surface_depth_multiplier
        }
        Cond::Water { offset, surface_depth_multiplier, add_stone_depth } => {
            ctx.water_height == i32::MIN
                || ctx.block_y + if *add_stone_depth { ctx.stone_depth_above } else { 0 }
                    >= ctx.water_height
                        + *offset
                        + ctx.surface_depth * *surface_depth_multiplier
        }
        Cond::Temperature => {
            let climate = ctx.system.biome_climate[ctx.get_biome() as usize];
            climate.cold_enough_to_snow(
                ctx.block_x,
                ctx.block_y,
                ctx.block_z,
                ctx.system.sea_level,
            )
        }
        Cond::Steep => {
            let x = ctx.block_x & 15;
            let z = ctx.block_z & 15;
            let z_north = i32::max(z - 1, 0);
            let z_south = i32::min(z + 1, 15);
            let hm = &ctx.chunk.world_surface_wg;
            let height_north = hm[(z_north * 16 + x) as usize];
            let height_south = hm[(z_south * 16 + x) as usize];
            if height_south >= height_north + 4 {
                return true;
            }
            let x_west = i32::max(x - 1, 0);
            let x_east = i32::min(x + 1, 15);
            let height_west = hm[(z * 16 + x_west) as usize];
            let height_east = hm[(z * 16 + x_east) as usize];
            height_west >= height_east + 4
        }
        Cond::Hole => ctx.surface_depth <= 0,
        Cond::AbovePreliminarySurface => ctx.block_y >= ctx.min_surface_level(),
        Cond::StoneDepth { offset, add_surface_depth, secondary_depth_range, ceiling } => {
            let stone_depth =
                if *ceiling { ctx.stone_depth_below } else { ctx.stone_depth_above };
            let surface_depth = if *add_surface_depth { ctx.surface_depth } else { 0 };
            let secondary = if *secondary_depth_range == 0 {
                0
            } else {
                mth_map(
                    ctx.system.get_surface_secondary(ctx.block_x, ctx.block_z),
                    -1.0,
                    1.0,
                    0.0,
                    *secondary_depth_range as f64,
                ) as i32
            };
            stone_depth <= 1 + *offset + surface_depth + secondary
        }
        Cond::Not(inner) => !test_condition(inner, ctx),
    }
}

// ---------------------------------------------------------------------------
// The P5 generator facade
// ---------------------------------------------------------------------------

/// [`super::density::ParityGenerator`] plus biomes and surface: fills the
/// chunk (P2/P3), bakes the 3×3 biome neighborhood (P4), then applies the
/// surface rules (P5).
pub struct SurfacedGenerator {
    pub inner: super::density::ParityGenerator,
    pub source: MultiNoiseBiomeSource,
    pub sampler: Sampler,
    pub surface: SurfaceSystem,
}

impl SurfacedGenerator {
    pub fn new_overworld(seed: i64) -> Self {
        let data = VanillaWorldgenData::load_overworld();
        let random_state = RandomState::new_overworld(&data, seed);
        let source = MultiNoiseBiomeSource::overworld();
        let sampler = Sampler::new(&random_state);
        let surface = SurfaceSystem::new(&data, &random_state, &source, seed);
        Self {
            inner: super::density::ParityGenerator { random_state },
            source,
            sampler,
            surface,
        }
    }

    /// Noise fill + surface for one chunk.
    pub fn generate_chunk(&self, chunk_x: i32, chunk_z: i32) -> FilledChunk {
        let mut chunk = self.inner.fill_chunk(chunk_x, chunk_z);
        let mut noise_chunk =
            NoiseChunk::for_chunk(&self.inner.random_state, chunk_x * 16, chunk_z * 16);
        let biomes = BakedBiomes::bake(
            &self.source,
            &self.sampler,
            chunk_x,
            chunk_z,
            chunk.min_y,
            chunk.height,
        );
        self.surface.build_surface(&mut chunk, &mut noise_chunk, &biomes, chunk_x, chunk_z);
        chunk
    }
}

fn try_apply(rule: &Rule, ctx: &mut ColumnCtx) -> Option<ParityBlock> {
    match rule {
        Rule::Block(state) => Some(*state),
        Rule::Sequence(rules) => rules.iter().find_map(|r| try_apply(r, ctx)),
        Rule::Test { condition, then_run } => {
            if test_condition(condition, ctx) {
                try_apply(then_run, ctx)
            } else {
                None
            }
        }
        Rule::Bandlands => Some(ctx.system.get_band(ctx.block_x, ctx.block_y, ctx.block_z)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaced_chunk_is_plausible() {
        let generator = SurfacedGenerator::new_overworld(8000);
        let chunk = generator.generate_chunk(0, 0);
        // Bedrock at the floor, air at the ceiling.
        assert_eq!(chunk.block(0, -64, 0), ParityBlock::Bedrock);
        assert_eq!(chunk.block(8, 319, 8), ParityBlock::Air);
        // Deepslate replaced the default block somewhere deep, and some
        // surface block (grass/dirt/sand/gravel/...) sits on top of a column.
        let mut deepslate = 0;
        let mut topsoil = 0;
        for z in 0..16 {
            for x in 0..16 {
                if chunk.block(x, -40, z) == ParityBlock::Deepslate {
                    deepslate += 1;
                }
                let top = chunk.world_surface_wg[(z * 16 + x) as usize] - 1;
                match chunk.block(x, top, z) {
                    ParityBlock::GrassBlock
                    | ParityBlock::Dirt
                    | ParityBlock::Sand
                    | ParityBlock::Gravel
                    | ParityBlock::SnowBlock
                    | ParityBlock::Water
                    | ParityBlock::Ice => topsoil += 1,
                    _ => {}
                }
            }
        }
        assert!(deepslate > 200, "deepslate coverage {deepslate}");
        assert!(topsoil > 128, "topsoil coverage {topsoil}");
    }

    /// The fixture column alphabet, indexed by `ParityBlock` discriminant —
    /// shared verbatim with `VelaP5Harness`.
    const BLOCK_ALPHABET: &str = "._~LgcCtiIBDGdepmusSrRvanfwWxXTkoybql";

    fn block_char(b: ParityBlock) -> char {
        BLOCK_ALPHABET.as_bytes()[b as usize] as char
    }

    /// Bit-for-bit parity against the reference JVM (`VelaP5Harness` on the
    /// real 26.2 server classes: real ProtoChunks, real
    /// `SurfaceSystem.buildSurface`): per seed the obfuscated zoom seed and
    /// clay-band array, and per chunk (47 across 3 seeds, incl. eroded
    /// badlands, badlands, frozen/deep-frozen oceans, swamps, snowy slopes,
    /// ice spikes, deserts, mushroom fields) the full block digest, sample
    /// columns, and both worldgen heightmaps after surfacing. Chunks are
    /// replayed in fixture order with one generator per seed so the RTree's
    /// last-result state evolves as in the dump.
    #[test]
    fn jvm_golden_parity_p5() {
        let fixture = include_str!("testdata/p5_golden.txt");
        // Generating the 47 chunks is the whole cost; the column/heightmap lines are
        // cheap lookups into already-built chunks. The biome source's RTree carries a
        // `Cell<last_result>` that evolves with query order, so a seed's lines must
        // replay strictly in fixture order — but the three seeds are fully independent
        // generators, so fan them out across threads (each seed owns its generator and
        // chunk cache). Preserves exact per-seed semantics at ~1/3 the wall time.
        let mut by_seed: Vec<(i64, Vec<&str>)> = Vec::new();
        for line in fixture.lines() {
            let seed: i64 =
                line.split_whitespace().nth(1).expect("seed").parse().expect("seed");
            match by_seed.iter_mut().find(|(s, _)| *s == seed) {
                Some((_, lines)) => lines.push(line),
                None => by_seed.push((seed, vec![line])),
            }
        }
        let checked: usize = std::thread::scope(|scope| {
            let handles: Vec<_> =
                by_seed.iter().map(|(_, lines)| scope.spawn(|| replay_seed(lines))).collect();
            handles.into_iter().map(|h| h.join().expect("seed replay thread")).sum()
        });
        assert_eq!(checked, 288, "fixture line count");
    }

    /// Replay one seed's slice of the golden fixture (in order) against a freshly
    /// built generator, asserting every line. Returns the number of lines checked.
    fn replay_seed(lines: &[&str]) -> usize {
        let mut generator: Option<SurfacedGenerator> = None;
        let mut chunks: HashMap<(i32, i32), FilledChunk> = HashMap::new();
        for line in lines {
            let mut parts = line.split_whitespace();
            let tag = parts.next().expect("tag");
            let seed: i64 = parts.next().expect("seed").parse().expect("seed");
            let generator =
                generator.get_or_insert_with(|| SurfacedGenerator::new_overworld(seed));
            match tag {
                "zoomseed" => {
                    let want: i64 = parts.next().unwrap().parse().unwrap();
                    assert_eq!(obfuscate_seed(seed), want, "zoomseed {seed}");
                }
                "clay" => {
                    let want = parts.next().expect("bands");
                    let got: String =
                        generator.surface.clay_bands.iter().map(|&b| block_char(b)).collect();
                    assert_eq!(got, want, "clay bands seed {seed}");
                }
                "chunk" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let digest = u64::from_str_radix(parts.next().expect("digest"), 16).unwrap();
                    let chunk = generator.generate_chunk(cx, cz);
                    let mut h = 0xcbf29ce484222325u64;
                    for b in &chunk.blocks {
                        h ^= *b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                    assert_eq!(h, digest, "chunk digest seed {seed} chunk ({cx},{cz})");
                    chunks.insert((cx, cz), chunk);
                }
                "column" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let x: i32 = parts.next().unwrap().parse().unwrap();
                    let z: i32 = parts.next().unwrap().parse().unwrap();
                    let want = parts.next().expect("column blocks");
                    let chunk = &chunks[&(cx, cz)];
                    let got: String = (0..chunk.height)
                        .map(|dy| block_char(chunk.block(x, chunk.min_y + dy, z)))
                        .collect();
                    assert_eq!(got, want, "column seed {seed} chunk ({cx},{cz}) at ({x},{z})");
                }
                "hmws" | "hmof" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let want = parts.next().expect("heights");
                    let chunk = &chunks[&(cx, cz)];
                    let hm =
                        if tag == "hmws" { &chunk.world_surface_wg } else { &chunk.ocean_floor_wg };
                    let got = (0..256)
                        .map(|i| {
                            let (z, x) = (i / 16, i % 16);
                            hm[(z * 16 + x) as usize].to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    assert_eq!(got, want, "{tag} seed {seed} chunk ({cx},{cz})");
                }
                other => panic!("unknown fixture tag {other}"),
            }
        }
        lines.len()
    }
}
