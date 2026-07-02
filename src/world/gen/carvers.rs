//! P7 — Carvers (Layer 6): procedural caves (`CaveWorldCarver`) and ravines
//! (`CanyonWorldCarver`), aquifer-coupled.
//!
//! Reference (ported 1:1, preserving the float/double widths of every
//! intermediate so the RNG-fed geometry matches bit-for-bit):
//! `world/level/levelgen/carver/` — `WorldCarver.carveEllipsoid`/`carveBlock`/
//! `getCarveState`/`canReach`, `CaveWorldCarver`, `CanyonWorldCarver`,
//! `CarverConfiguration`; `NoiseBasedChunkGenerator.applyCarvers`;
//! `world/level/chunk/CarvingMask`; the value providers
//! (`UniformFloat`/`TrapezoidFloat`/`ConstantFloat`/`UniformHeight`),
//! `VerticalAnchor`, and `Mth.sin`/`cos`/`floor`. The built-in configured
//! carvers are transcribed from `data/worldgen/Carvers.java` (the overworld
//! `cave`, `cave_extra_underground`, `canyon`); the per-biome carver *lists*
//! (and their order, which feeds `setLargeFeatureSeed(seed + index, …)`) are
//! read data-first from the vendored biome JSON under `data/…/worldgen/biome/`.
//!
//! Seeding (`applyCarvers`): for every source chunk in the 17×17 (`range 8`)
//! neighborhood of the target, re-seed one legacy-LCG `WorldgenRandom` with
//! `setLargeFeatureSeed(seed + carverIndex, srcX, srcZ)`, so a carver *started*
//! in a neighbor deterministically reaches across the border into the center
//! chunk — the only chunk written (write radius 0). Carved blocks are resolved
//! through the center chunk's aquifer (`NoiseChunk::carve_state`).
//!
//! Top-material fix-up: vanilla's `carveBlock` rewrites the dirt directly under
//! a carved grass/mycelium column to the biome top material (via
//! `SurfaceSystem.topMaterial`). This is ported 1:1 (`carve_block` +
//! `SurfaceSystem::top_material`): a per-column `hasGrass` latch, and when the
//! block below a carved opening is exposed `dirt` it is re-topped through the
//! surface rule at that single position (`stoneDepthAbove/Below = 1`,
//! `waterHeight = underFluid ? y+1 : MIN`). Everything is block-exact.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use super::density::{FilledChunk, NoiseChunk, ParityBlock};
use super::random::{RandomSource, WorldgenRandom};
use super::surface_rules::{BakedBiomes, SurfaceSystem, SurfacedGenerator};
use super::vanilla_jsons;

// ---------------------------------------------------------------------------
// Mth — the vanilla trig lookup table and floor (parity-critical: carvers walk
// on `Mth.sin`/`cos`, not `f64::sin`).
// ---------------------------------------------------------------------------

const SIN_SCALE: f64 = 10430.378350470453;

/// `Mth.SIN` — 65536-entry `(float)Math.sin(i / 10430.378350470453)` table.
fn sin_table() -> &'static [f32; 65536] {
    static TABLE: OnceLock<Box<[f32; 65536]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([0.0f32; 65536]);
        for (i, slot) in t.iter_mut().enumerate() {
            *slot = (i as f64 / SIN_SCALE).sin() as f32;
        }
        t
    })
}

/// `Mth.sin(double)`.
fn mth_sin(d: f64) -> f32 {
    sin_table()[((d * SIN_SCALE) as i64 & 65535) as usize]
}

/// `Mth.cos(double)`.
fn mth_cos(d: f64) -> f32 {
    sin_table()[((d * SIN_SCALE + 16384.0) as i64 & 65535) as usize]
}

/// `Mth.floor(double)`.
fn mth_floor(d: f64) -> i32 {
    let i = d as i64;
    (if d < i as f64 { i - 1 } else { i }) as i32
}

// ---------------------------------------------------------------------------
// Value providers, vertical anchors, height providers
// ---------------------------------------------------------------------------

/// The subset of `RandomSource` the value providers need. Implemented for both
/// the outer `WorldgenRandom` (used in `carve`) and the per-tunnel legacy
/// `RandomSource` (used in the canyon walk), matching vanilla where both call
/// sites feed the same `FloatProvider.sample`.
trait CarveRng {
    fn next_float(&mut self) -> f32;
    fn next_int_bounded(&mut self, bound: i32) -> i32;
}

impl CarveRng for WorldgenRandom {
    fn next_float(&mut self) -> f32 {
        WorldgenRandom::next_float(self)
    }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        WorldgenRandom::next_int_bounded(self, bound)
    }
}

impl CarveRng for RandomSource {
    fn next_float(&mut self) -> f32 {
        RandomSource::next_float(self)
    }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        RandomSource::next_int_bounded(self, bound)
    }
}

/// `FloatProvider` — the three kinds carvers reference.
#[derive(Clone, Copy)]
enum FloatProvider {
    Constant(f32),
    /// `UniformFloat` — `[min, max)`.
    Uniform { min: f32, max: f32 },
    /// `TrapezoidFloat`.
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatProvider {
    fn sample<R: CarveRng>(&self, r: &mut R) -> f32 {
        match *self {
            FloatProvider::Constant(v) => v,
            // `Mth.randomBetween` = nextFloat() * (max - min) + min.
            FloatProvider::Uniform { min, max } => r.next_float() * (max - min) + min,
            FloatProvider::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let plateau_start = (range - plateau) / 2.0;
                let plateau_end = range - plateau_start;
                // Two draws, left to right.
                let a = r.next_float();
                let b = r.next_float();
                min + a * plateau_end + b * plateau_start
            }
        }
    }
}

/// `VerticalAnchor`.
#[derive(Clone, Copy)]
enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    /// Faithful to `VerticalAnchor.BelowTop`; used by the nether cave carver,
    /// which is not wired into the overworld pipeline yet.
    #[allow(dead_code)]
    BelowTop(i32),
}

impl VerticalAnchor {
    fn resolve_y(self, min_gen_y: i32, gen_depth: i32) -> i32 {
        match self {
            VerticalAnchor::Absolute(y) => y,
            VerticalAnchor::AboveBottom(o) => min_gen_y + o,
            VerticalAnchor::BelowTop(o) => gen_depth - 1 + min_gen_y - o,
        }
    }
}

/// `UniformHeight` (the only `HeightProvider` carvers use).
#[derive(Clone, Copy)]
struct UniformHeight {
    min: VerticalAnchor,
    max: VerticalAnchor,
}

impl UniformHeight {
    fn sample<R: CarveRng>(&self, r: &mut R, min_gen_y: i32, gen_depth: i32) -> i32 {
        let min = self.min.resolve_y(min_gen_y, gen_depth);
        let max = self.max.resolve_y(min_gen_y, gen_depth);
        if min > max {
            min
        } else {
            // `Mth.randomBetweenInclusive` = nextInt(max - min + 1) + min.
            r.next_int_bounded(max - min + 1) + min
        }
    }
}

// ---------------------------------------------------------------------------
// Configured carvers
// ---------------------------------------------------------------------------

struct CaveConfig {
    probability: f32,
    y: UniformHeight,
    y_scale: FloatProvider,
    lava_level: VerticalAnchor,
    horizontal_radius_multiplier: FloatProvider,
    vertical_radius_multiplier: FloatProvider,
    floor_level: FloatProvider,
}

struct CanyonConfig {
    probability: f32,
    y: UniformHeight,
    y_scale: FloatProvider,
    lava_level: VerticalAnchor,
    vertical_rotation: FloatProvider,
    distance_factor: FloatProvider,
    thickness: FloatProvider,
    width_smoothness: i32,
    horizontal_radius_factor: FloatProvider,
    vertical_radius_default_factor: f32,
    vertical_radius_center_factor: f32,
}

enum ConfiguredCarver {
    Cave(CaveConfig),
    Canyon(CanyonConfig),
}

impl ConfiguredCarver {
    fn probability(&self) -> f32 {
        match self {
            ConfiguredCarver::Cave(c) => c.probability,
            ConfiguredCarver::Canyon(c) => c.probability,
        }
    }

    /// `WorldCarver.isStartChunk` — `random.nextFloat() <= probability`.
    fn is_start_chunk(&self, r: &mut WorldgenRandom) -> bool {
        r.next_float() <= self.probability()
    }
}

/// The overworld built-in configured carvers (`data/worldgen/Carvers.java`),
/// each paired with its registry id (without the `minecraft:` prefix).
fn built_in_carvers() -> Vec<(&'static str, ConfiguredCarver)> {
    use FloatProvider::*;
    use VerticalAnchor::*;
    let cave_shape = |probability, y_max: VerticalAnchor| CaveConfig {
        probability,
        y: UniformHeight { min: AboveBottom(8), max: y_max },
        y_scale: Uniform { min: 0.1, max: 0.9 },
        lava_level: AboveBottom(8),
        horizontal_radius_multiplier: Uniform { min: 0.7, max: 1.4 },
        vertical_radius_multiplier: Uniform { min: 0.8, max: 1.3 },
        floor_level: Uniform { min: -1.0, max: -0.4 },
    };
    vec![
        ("cave", ConfiguredCarver::Cave(cave_shape(0.15, Absolute(180)))),
        (
            "cave_extra_underground",
            ConfiguredCarver::Cave(cave_shape(0.07, Absolute(47))),
        ),
        (
            "canyon",
            ConfiguredCarver::Canyon(CanyonConfig {
                probability: 0.01,
                y: UniformHeight { min: Absolute(10), max: Absolute(67) },
                y_scale: Constant(3.0),
                lava_level: AboveBottom(8),
                vertical_rotation: Uniform { min: -0.125, max: 0.125 },
                distance_factor: Uniform { min: 0.75, max: 1.0 },
                thickness: Trapezoid { min: 0.0, max: 6.0, plateau: 2.0 },
                width_smoothness: 3,
                horizontal_radius_factor: Uniform { min: 0.75, max: 1.0 },
                vertical_radius_default_factor: 1.0,
                vertical_radius_center_factor: 0.0,
            }),
        ),
    ]
}

/// The carver registry plus the per-biome carver lists, built once.
struct CarverRegistry {
    carvers: Vec<ConfiguredCarver>,
    /// Bare biome name → carver indices in the biome's declared order. `None`
    /// entries preserve the list slot (hence the carver index) for a carver id
    /// not among the overworld built-ins — never hit for overworld biomes, but
    /// kept faithful so `setLargeFeatureSeed(seed + index, …)` stays aligned.
    by_biome: HashMap<String, Vec<Option<usize>>>,
}

fn carver_registry() -> &'static CarverRegistry {
    static REG: OnceLock<CarverRegistry> = OnceLock::new();
    REG.get_or_init(|| {
        let built = built_in_carvers();
        let index_of: HashMap<&str, usize> =
            built.iter().enumerate().map(|(i, (id, _))| (*id, i)).collect();
        let carvers: Vec<ConfiguredCarver> = built.into_iter().map(|(_, c)| c).collect();

        let mut by_biome = HashMap::new();
        for (name, json) in vanilla_jsons::BIOMES {
            let value: Value = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("biome {name} json: {e}"));
            let list = match value.get("carvers") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|id| index_of.get(strip_ns(id)).copied())
                    .collect(),
                Some(Value::String(s)) => {
                    vec![index_of.get(strip_ns(s)).copied()]
                }
                _ => Vec::new(),
            };
            by_biome.insert((*name).to_string(), list);
        }
        CarverRegistry { carvers, by_biome }
    })
}

fn strip_ns(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

// ---------------------------------------------------------------------------
// CarvingMask
// ---------------------------------------------------------------------------

/// `world/level/chunk/CarvingMask` — a per-chunk bitset over
/// `x&15 | (z&15)<<4 | (y-minY)<<8`.
struct CarvingMask {
    min_y: i32,
    bits: Vec<u64>,
}

impl CarvingMask {
    fn new(height: i32, min_y: i32) -> Self {
        let words = ((256 * height) as usize).div_ceil(64);
        Self { min_y, bits: vec![0u64; words] }
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((x & 15) | ((z & 15) << 4) | ((y - self.min_y) << 8)) as usize
    }

    fn set(&mut self, x: i32, y: i32, z: i32) {
        let i = self.index(x, y, z);
        self.bits[i >> 6] |= 1u64 << (i & 63);
    }

    fn get(&self, x: i32, y: i32, z: i32) -> bool {
        let i = self.index(x, y, z);
        (self.bits[i >> 6] >> (i & 63)) & 1 != 0
    }
}

// ---------------------------------------------------------------------------
// Carver-replaceable predicate (`#minecraft:overworld_carver_replaceables`,
// resolved over the parity-block alphabet). See the tag expansion in
// `data/…/tags/block/overworld_carver_replaceables.json`.
// ---------------------------------------------------------------------------

/// `WorldCarver.canReplaceBlock` for the overworld carvers. The vanilla tag
/// (recursively: base_stone_overworld, substrate_overworld, sand, terracotta,
/// iron/copper ores, snow, plus water/gravel/sandstones/calcite/packed_ice/raw
/// ore blocks/cinnabar/sulfur…) intersected with the blocks Vela's terrain +
/// surface can produce. Notably excludes air, lava, bedrock, and plain ice.
fn is_carver_replaceable(b: ParityBlock) -> bool {
    use ParityBlock::*;
    matches!(
        b,
        Stone
            | Granite
            | Tuff
            | Deepslate
            | Dirt
            | CoarseDirt
            | Mud
            | GrassBlock
            | Podzol
            | Mycelium
            | Sand
            | RedSand
            | Sandstone
            | RedSandstone
            | Gravel
            | Calcite
            | PackedIce
            | SnowBlock
            | PowderSnow
            | Water
            | CopperOre
            | DeepslateIronOre
            | RawCopperBlock
            | RawIronBlock
            | Cinnabar
            | Sulfur
            | Terracotta
            | WhiteTerracotta
            | OrangeTerracotta
            | YellowTerracotta
            | BrownTerracotta
            | RedTerracotta
            | LightGrayTerracotta
    )
}

// ---------------------------------------------------------------------------
// The carve environment + shared geometry
// ---------------------------------------------------------------------------

/// The mutable view a carve runs against: the center chunk's blocks, its
/// aquifer-bearing `NoiseChunk`, and the shared carving mask.
struct Env<'a> {
    blocks: &'a mut FilledChunk,
    noise: &'a mut NoiseChunk,
    mask: &'a mut CarvingMask,
    /// The surface system + baked 3×3 biomes, for the `carveBlock` top-material
    /// fix-up (dirt exposed under a carved grass/mycelium column → biome top).
    surface: &'a SurfaceSystem,
    biomes: &'a BakedBiomes,
    min_gen_y: i32,
    gen_depth: i32,
    /// The center chunk position (`ChunkPos` of the chunk being carved).
    center: (i32, i32),
}

/// `WorldCarver.CarveSkipChecker` — the per-carver ellipsoid envelope test.
enum SkipChecker {
    /// `CaveWorldCarver.shouldSkip`.
    Cave { floor_level: f64 },
    /// `CanyonWorldCarver.shouldSkip` (per-height width factors).
    Canyon { width_factors: Vec<f32> },
}

impl SkipChecker {
    fn should_skip(&self, xd: f64, yd: f64, zd: f64, world_y: i32, min_gen_y: i32) -> bool {
        match self {
            SkipChecker::Cave { floor_level } => {
                if yd <= *floor_level {
                    true
                } else {
                    xd * xd + yd * yd + zd * zd >= 1.0
                }
            }
            SkipChecker::Canyon { width_factors } => {
                let y_index = world_y - min_gen_y;
                (xd * xd + zd * zd) * width_factors[(y_index - 1) as usize] as f64 + yd * yd / 6.0
                    >= 1.0
            }
        }
    }
}

/// `WorldCarver.canReach`.
fn can_reach(center: (i32, i32), x: f64, z: f64, current_step: i32, total_steps: i32, thickness: f32) -> bool {
    let x_mid = (center.0 * 16 + 8) as f64;
    let z_mid = (center.1 * 16 + 8) as f64;
    let xd = x - x_mid;
    let zd = z - z_mid;
    let remaining = (total_steps - current_step) as f64;
    let rr = (thickness + 2.0 + 16.0) as f64;
    xd * xd + zd * zd - remaining * remaining <= rr * rr
}

/// `WorldCarver.carveEllipsoid`.
#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(
    env: &mut Env,
    lava_level: i32,
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
    skip: &SkipChecker,
) -> bool {
    let (cx, cz) = env.center;
    let center_x = (cx * 16 + 8) as f64;
    let center_z = (cz * 16 + 8) as f64;
    let max_delta = 16.0 + horizontal_radius * 2.0;
    if (x - center_x).abs() > max_delta || (z - center_z).abs() > max_delta {
        return false;
    }
    let chunk_min_x = cx * 16;
    let chunk_min_z = cz * 16;
    let min_x_index = (mth_floor(x - horizontal_radius) - chunk_min_x - 1).max(0);
    let max_x_index = (mth_floor(x + horizontal_radius) - chunk_min_x).min(15);
    let min_y = (mth_floor(y - vertical_radius) - 1).max(env.min_gen_y + 1);
    // `protectedBlocksOnTop` — 7 for a non-upgrading chunk (always, here).
    let protected_top = 7;
    let max_y = (mth_floor(y + vertical_radius) + 1).min(env.min_gen_y + env.gen_depth - 1 - protected_top);
    let min_z_index = (mth_floor(z - horizontal_radius) - chunk_min_z - 1).max(0);
    let max_z_index = (mth_floor(z + horizontal_radius) - chunk_min_z).min(15);
    let mut carved = false;
    for x_index in min_x_index..=max_x_index {
        let world_x = chunk_min_x + x_index;
        let xd = (world_x as f64 + 0.5 - x) / horizontal_radius;
        for z_index in min_z_index..=max_z_index {
            let world_z = chunk_min_z + z_index;
            let zd = (world_z as f64 + 0.5 - z) / horizontal_radius;
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            // `MutableBoolean hasGrass` — per column, latched once a carved block
            // is grass/mycelium so the dirt below the opening is re-topped.
            let mut has_grass = false;
            let mut world_y = max_y;
            while world_y > min_y {
                let yd = (world_y as f64 - 0.5 - y) / vertical_radius;
                if !skip.should_skip(xd, yd, zd, world_y, env.min_gen_y)
                    && !env.mask.get(x_index, world_y, z_index)
                {
                    env.mask.set(x_index, world_y, z_index);
                    if carve_block(env, lava_level, world_x, world_y, world_z, &mut has_grass) {
                        carved = true;
                    }
                }
                world_y -= 1;
            }
        }
    }
    carved
}

/// `WorldCarver.carveBlock`. Reads the current block from the center chunk,
/// checks the replaceable tag, resolves the carve state through the aquifer,
/// writes it back, and — when the just-carved column was grass/mycelium and the
/// block directly below is now exposed dirt — rewrites that dirt to the biome's
/// top material (`SurfaceSystem.topMaterial`), exactly like vanilla.
fn carve_block(
    env: &mut Env,
    lava_level: i32,
    wx: i32,
    wy: i32,
    wz: i32,
    has_grass: &mut bool,
) -> bool {
    let lx = wx & 15;
    let lz = wz & 15;
    let current = env.blocks.block(lx, wy, lz);
    // Latch `hasGrass` before the replaceable check (vanilla notes it on the
    // block about to be carved, whether or not it ends up replaced).
    if current == ParityBlock::GrassBlock || current == ParityBlock::Mycelium {
        *has_grass = true;
    }
    if !is_carver_replaceable(current) {
        return false;
    }
    let Some(state) = env.noise.carve_state(wx, wy, wz, lava_level) else {
        return false;
    };
    env.blocks.set_block(lx, wy, lz, state);
    // Top-material fix-up: if this column exposed dirt directly beneath a carved
    // grass/mycelium block, re-top it to the biome surface (grass under grass,
    // mycelium under mushroom fields, …). `underFluid` is whether the block we
    // just carved is now a fluid (water/lava), which selects the surface rule's
    // submerged branch.
    if *has_grass {
        let below_y = wy - 1;
        if env.blocks.block(lx, below_y, lz) == ParityBlock::Dirt {
            let under_fluid = state.is_fluid();
            if let Some(top) = env.surface.top_material(
                &mut *env.blocks,
                &mut *env.noise,
                env.biomes,
                wx,
                below_y,
                wz,
                under_fluid,
            ) {
                env.blocks.set_block(lx, below_y, lz, top);
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// CaveWorldCarver
// ---------------------------------------------------------------------------

/// `SectionPos.sectionToBlockCoord(getRange() * 2 - 1)` with `getRange() == 4`.
const CAVE_MAX_DISTANCE: i32 = (4 * 2 - 1) << 4;

fn cave_carve(cfg: &CaveConfig, env: &mut Env, source: (i32, i32), random: &mut WorldgenRandom) {
    let (min_gen_y, gen_depth) = (env.min_gen_y, env.gen_depth);
    let lava_level = cfg.lava_level.resolve_y(min_gen_y, gen_depth);
    // `random.nextInt(random.nextInt(random.nextInt(getCaveBound()) + 1) + 1)`.
    let a = random.next_int_bounded(15);
    let b = random.next_int_bounded(a + 1);
    let cave_count = random.next_int_bounded(b + 1);

    let source_min_x = source.0 * 16;
    let source_min_z = source.1 * 16;
    for _cave in 0..cave_count {
        let x = (source_min_x + random.next_int_bounded(16)) as f64;
        let y = cfg.y.sample(random, min_gen_y, gen_depth) as f64;
        let z = (source_min_z + random.next_int_bounded(16)) as f64;
        let hrm = cfg.horizontal_radius_multiplier.sample(random) as f64;
        let vrm = cfg.vertical_radius_multiplier.sample(random) as f64;
        let floor_level = cfg.floor_level.sample(random) as f64;
        let skip = SkipChecker::Cave { floor_level };
        let mut tunnels = 1;
        if random.next_int_bounded(4) == 0 {
            let y_scale = cfg.y_scale.sample(random) as f64;
            let thickness = 1.0 + random.next_float() * 6.0;
            create_room(env, lava_level, x, y, z, thickness, y_scale, &skip);
            tunnels += random.next_int_bounded(4);
        }
        for _i in 0..tunnels {
            let h_rot = random.next_float() * std::f32::consts::TAU;
            let v_rot = (random.next_float() - 0.5) / 4.0;
            let thickness = cave_thickness(random);
            let distance = CAVE_MAX_DISTANCE - random.next_int_bounded(CAVE_MAX_DISTANCE / 4);
            let seed = random.next_long();
            create_tunnel(
                env, lava_level, seed, x, y, z, hrm, vrm, thickness, h_rot, v_rot, 0, distance, 1.0,
                &skip,
            );
        }
    }
}

/// `CaveWorldCarver.getThickness`.
fn cave_thickness(r: &mut WorldgenRandom) -> f32 {
    let a = r.next_float();
    let b = r.next_float();
    let mut thickness = a * 2.0 + b;
    if r.next_int_bounded(10) == 0 {
        let c = r.next_float();
        let d = r.next_float();
        thickness *= c * d * 3.0 + 1.0;
    }
    thickness
}

/// `CaveWorldCarver.createRoom`.
#[allow(clippy::too_many_arguments)]
fn create_room(
    env: &mut Env,
    lava_level: i32,
    x: f64,
    y: f64,
    z: f64,
    thickness: f32,
    y_scale: f64,
    skip: &SkipChecker,
) {
    let horizontal_radius = 1.5 + (mth_sin(std::f32::consts::FRAC_PI_2 as f64) * thickness) as f64;
    let vertical_radius = horizontal_radius * y_scale;
    carve_ellipsoid(env, lava_level, x + 1.0, y, z, horizontal_radius, vertical_radius, skip);
}

/// `CaveWorldCarver.createTunnel`.
#[allow(clippy::too_many_arguments)]
fn create_tunnel(
    env: &mut Env,
    lava_level: i32,
    tunnel_seed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    hrm: f64,
    vrm: f64,
    thickness: f32,
    mut h_rot: f32,
    mut v_rot: f32,
    step: i32,
    dist: i32,
    y_scale: f64,
    skip: &SkipChecker,
) {
    let mut random = RandomSource::legacy(tunnel_seed);
    let split_point = random.next_int_bounded(dist / 2) + dist / 4;
    let steep = random.next_int_bounded(6) == 0;
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;

    for current_step in step..dist {
        let horizontal_radius = 1.5
            + (mth_sin((std::f32::consts::PI * current_step as f32 / dist as f32) as f64) * thickness)
                as f64;
        let vertical_radius = horizontal_radius * y_scale;
        let cos_x = mth_cos(v_rot as f64);
        x += (mth_cos(h_rot as f64) * cos_x) as f64;
        y += mth_sin(v_rot as f64) as f64;
        z += (mth_sin(h_rot as f64) * cos_x) as f64;
        v_rot *= if steep { 0.92 } else { 0.7 };
        v_rot += x_rota * 0.1;
        h_rot += y_rota * 0.1;
        x_rota *= 0.9;
        y_rota *= 0.75;
        {
            let a = random.next_float();
            let b = random.next_float();
            let c = random.next_float();
            x_rota += (a - b) * c * 2.0;
        }
        {
            let a = random.next_float();
            let b = random.next_float();
            let c = random.next_float();
            y_rota += (a - b) * c * 4.0;
        }
        if current_step == split_point && thickness > 1.0 {
            let s1 = random.next_long();
            let t1 = random.next_float() * 0.5 + 0.5;
            create_tunnel(
                env, lava_level, s1, x, y, z, hrm, vrm, t1,
                h_rot - std::f32::consts::FRAC_PI_2, v_rot / 3.0, current_step, dist, 1.0, skip,
            );
            let s2 = random.next_long();
            let t2 = random.next_float() * 0.5 + 0.5;
            create_tunnel(
                env, lava_level, s2, x, y, z, hrm, vrm, t2,
                h_rot + std::f32::consts::FRAC_PI_2, v_rot / 3.0, current_step, dist, 1.0, skip,
            );
            return;
        }
        if random.next_int_bounded(4) != 0 {
            if !can_reach(env.center, x, z, current_step, dist, thickness) {
                return;
            }
            carve_ellipsoid(
                env,
                lava_level,
                x,
                y,
                z,
                horizontal_radius * hrm,
                vertical_radius * vrm,
                skip,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CanyonWorldCarver
// ---------------------------------------------------------------------------

/// `(getRange() * 2 - 1) * 16` with `getRange() == 4`.
const CANYON_MAX_DISTANCE: i32 = (4 * 2 - 1) * 16;

fn canyon_carve(cfg: &CanyonConfig, env: &mut Env, source: (i32, i32), random: &mut WorldgenRandom) {
    let (min_gen_y, gen_depth) = (env.min_gen_y, env.gen_depth);
    let lava_level = cfg.lava_level.resolve_y(min_gen_y, gen_depth);
    let source_min_x = source.0 * 16;
    let source_min_z = source.1 * 16;

    let x = (source_min_x + random.next_int_bounded(16)) as f64;
    let y = cfg.y.sample(random, min_gen_y, gen_depth);
    let z = (source_min_z + random.next_int_bounded(16)) as f64;
    let h_rot = random.next_float() * std::f32::consts::TAU;
    let v_rot = cfg.vertical_rotation.sample(random);
    let y_scale = cfg.y_scale.sample(random) as f64;
    let thickness = cfg.thickness.sample(random);
    let distance = (CANYON_MAX_DISTANCE as f32 * cfg.distance_factor.sample(random)) as i32;
    let seed = random.next_long();
    canyon_do_carve(
        cfg, env, lava_level, seed, x, y as f64, z, thickness, h_rot, v_rot, 0, distance, y_scale,
    );
}

/// `CanyonWorldCarver.doCarve`.
#[allow(clippy::too_many_arguments)]
fn canyon_do_carve(
    cfg: &CanyonConfig,
    env: &mut Env,
    lava_level: i32,
    tunnel_seed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    thickness: f32,
    mut h_rot: f32,
    mut v_rot: f32,
    step: i32,
    distance: i32,
    y_scale: f64,
) {
    let mut random = RandomSource::legacy(tunnel_seed);
    let width_factors = canyon_init_width_factors(cfg, env.gen_depth, &mut random);
    let skip = SkipChecker::Canyon { width_factors };
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;

    for current_step in step..distance {
        let mut horizontal_radius = 1.5
            + (mth_sin((current_step as f32 * std::f32::consts::PI / distance as f32) as f64)
                * thickness) as f64;
        let mut vertical_radius = horizontal_radius * y_scale;
        horizontal_radius *= cfg.horizontal_radius_factor.sample(&mut random) as f64;
        vertical_radius = canyon_update_vertical_radius(cfg, &mut random, vertical_radius, distance, current_step);
        let xc = mth_cos(v_rot as f64);
        let xs = mth_sin(v_rot as f64);
        x += (mth_cos(h_rot as f64) * xc) as f64;
        y += xs as f64;
        z += (mth_sin(h_rot as f64) * xc) as f64;
        v_rot *= 0.7;
        v_rot += x_rota * 0.05;
        h_rot += y_rota * 0.05;
        x_rota *= 0.8;
        y_rota *= 0.5;
        {
            let a = random.next_float();
            let b = random.next_float();
            let c = random.next_float();
            x_rota += (a - b) * c * 2.0;
        }
        {
            let a = random.next_float();
            let b = random.next_float();
            let c = random.next_float();
            y_rota += (a - b) * c * 4.0;
        }
        if random.next_int_bounded(4) != 0 {
            if !can_reach(env.center, x, z, current_step, distance, thickness) {
                return;
            }
            carve_ellipsoid(env, lava_level, x, y, z, horizontal_radius, vertical_radius, &skip);
        }
    }
}

/// `CanyonWorldCarver.initWidthFactors`.
fn canyon_init_width_factors(cfg: &CanyonConfig, gen_depth: i32, random: &mut RandomSource) -> Vec<f32> {
    let depth = gen_depth as usize;
    let mut factors = Vec::with_capacity(depth);
    let mut width_factor = 1.0f32;
    for y_index in 0..depth {
        // `yIndex == 0 || random.nextInt(widthSmoothness) == 0` — short-circuit
        // keeps the RNG untouched at yIndex 0, matching vanilla.
        if y_index == 0 || random.next_int_bounded(cfg.width_smoothness) == 0 {
            let a = random.next_float();
            let b = random.next_float();
            width_factor = 1.0 + a * b;
        }
        factors.push(width_factor * width_factor);
    }
    factors
}

/// `CanyonWorldCarver.updateVerticalRadius`.
fn canyon_update_vertical_radius(
    cfg: &CanyonConfig,
    random: &mut RandomSource,
    vertical_radius: f64,
    distance: i32,
    current_step: i32,
) -> f64 {
    let vertical_multiplier =
        1.0f32 - (0.5 - current_step as f32 / distance as f32).abs() * 2.0;
    let factor = cfg.vertical_radius_default_factor + cfg.vertical_radius_center_factor * vertical_multiplier;
    // `Mth.randomBetween(random, 0.75F, 1.0F)`.
    let rb = random.next_float() * 0.25 + 0.75;
    factor as f64 * vertical_radius * rb as f64
}

// ---------------------------------------------------------------------------
// applyCarvers
// ---------------------------------------------------------------------------

/// `NoiseBasedChunkGenerator.applyCarvers` for the center chunk `pos`. Mutates
/// `blocks` in place (write radius 0 — only the center chunk is written).
pub fn apply_carvers(
    generator: &SurfacedGenerator,
    seed: i64,
    pos: (i32, i32),
    blocks: &mut FilledChunk,
    biomes: &BakedBiomes,
) {
    let rs = &generator.inner.random_state;
    let ns = rs.settings.noise;
    let (min_gen_y, gen_depth) = (ns.min_y, ns.height);

    let mut noise = NoiseChunk::for_chunk(rs, pos.0 * 16, pos.1 * 16);
    let mut mask = CarvingMask::new(gen_depth, min_gen_y);
    let registry = carver_registry();
    // `new WorldgenRandom(new LegacyRandomSource(generateUniqueSeed()))` — the
    // initial seed is irrelevant, every carver re-seeds via setLargeFeatureSeed.
    let mut random = WorldgenRandom::new(RandomSource::legacy(0));

    let mut env = Env {
        blocks,
        noise: &mut noise,
        mask: &mut mask,
        surface: &generator.surface,
        biomes,
        min_gen_y,
        gen_depth,
        center: pos,
    };

    // `range = 8`.
    for dx in -8..=8 {
        for dz in -8..=8 {
            let source = (pos.0 + dx, pos.1 + dz);
            // `biomeSource.getNoiseBiome(QuartPos.fromBlock(sourceMinBlockX), 0,
            // QuartPos.fromBlock(sourceMinBlockZ), sampler)` — sampled fresh
            // from the biome source (vanilla's `carverBiome` supplier), not from
            // stored neighbor sections.
            let quart_x = (source.0 * 16) >> 2;
            let quart_z = (source.1 * 16) >> 2;
            let biome = generator.source.get_noise_biome(quart_x, 0, quart_z, &generator.sampler);
            let Some(list) = registry.by_biome.get(strip_ns(biome)) else {
                continue;
            };
            for (index, slot) in list.iter().enumerate() {
                random.set_large_feature_seed(seed + index as i64, source.0, source.1);
                if let Some(cidx) = slot {
                    let carver = &registry.carvers[*cidx];
                    if carver.is_start_chunk(&mut random) {
                        match carver {
                            ConfiguredCarver::Cave(c) => cave_carve(c, &mut env, source, &mut random),
                            ConfiguredCarver::Canyon(c) => {
                                canyon_carve(c, &mut env, source, &mut random)
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mth_trig_matches_lookup_definition() {
        // Table endpoints and a few interior probes against the vanilla formula.
        for &d in &[0.0f64, 0.5, 1.0, 1.5707963, 3.1415927, -2.0, 12.34] {
            let want_sin = sin_table()[((d * SIN_SCALE) as i64 & 65535) as usize];
            let want_cos = sin_table()[((d * SIN_SCALE + 16384.0) as i64 & 65535) as usize];
            assert_eq!(mth_sin(d), want_sin);
            assert_eq!(mth_cos(d), want_cos);
        }
        // sin(0) == 0, and cos is a quarter-phase-shifted sin.
        assert_eq!(mth_sin(0.0), 0.0);
        assert!((mth_cos(0.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn mth_floor_matches_vanilla() {
        assert_eq!(mth_floor(3.0), 3);
        assert_eq!(mth_floor(3.9), 3);
        assert_eq!(mth_floor(-0.5), -1);
        assert_eq!(mth_floor(-3.0), -3);
        assert_eq!(mth_floor(-3.1), -4);
    }

    #[test]
    fn carving_mask_index_and_bits() {
        let mut mask = CarvingMask::new(384, -64);
        assert!(!mask.get(3, 10, 5));
        mask.set(3, 10, 5);
        assert!(mask.get(3, 10, 5));
        // Distinct cells don't alias.
        assert!(!mask.get(4, 10, 5));
        assert!(!mask.get(3, 11, 5));
        // Index formula: x&15 | (z&15)<<4 | (y-minY)<<8.
        assert_eq!(mask.index(3, -64, 5), 3 | (5 << 4));
        assert_eq!(mask.index(0, -63, 0), 1 << 8);
    }

    #[test]
    fn registry_maps_overworld_biome_carvers() {
        let reg = carver_registry();
        // Plains carries the standard three carvers, in order.
        let plains = reg.by_biome.get("plains").expect("plains biome present");
        assert_eq!(plains.len(), 3);
        assert!(plains.iter().all(|s| s.is_some()));
        // The registry ids resolve to the expected kinds.
        assert!(matches!(reg.carvers[plains[0].unwrap()], ConfiguredCarver::Cave(_)));
        assert!(matches!(reg.carvers[plains[1].unwrap()], ConfiguredCarver::Cave(_)));
        assert!(matches!(reg.carvers[plains[2].unwrap()], ConfiguredCarver::Canyon(_)));
        // Probabilities transcribed from Carvers.java.
        assert_eq!(reg.carvers[plains[0].unwrap()].probability(), 0.15);
        assert_eq!(reg.carvers[plains[1].unwrap()].probability(), 0.07);
        assert_eq!(reg.carvers[plains[2].unwrap()].probability(), 0.01);
    }

    #[test]
    fn value_providers_match_reference_formulas() {
        // Constant is draw-free.
        let mut r = WorldgenRandom::new(RandomSource::legacy(1));
        assert_eq!(FloatProvider::Constant(3.0).sample(&mut r), 3.0);
        // Uniform in range.
        let u = FloatProvider::Uniform { min: 0.7, max: 1.4 };
        let v = u.sample(&mut r);
        assert!((0.7..1.4).contains(&v), "uniform out of range: {v}");
        // UniformHeight in range for the overworld anchors.
        let h = UniformHeight {
            min: VerticalAnchor::AboveBottom(8),
            max: VerticalAnchor::Absolute(180),
        };
        let y = h.sample(&mut r, -64, 384);
        assert!((-56..=180).contains(&y), "height out of range: {y}");
    }
}
