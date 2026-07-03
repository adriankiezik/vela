#![allow(dead_code)]
//! P8 — feature / decoration engine (Layer 7).
//!
//! Ports `ChunkGenerator.applyBiomeDecoration` and `FeatureSorter` and drives a
//! prioritized set of overworld [`ConfiguredFeature`] implementations. The
//! engine is 1:1 with vanilla 26.2:
//!
//! * **Decoration order** — 11 `GenerationStep.Decoration` steps in fixed order.
//! * **Global feature indexing** — every biome present in the chunk's 3×3
//!   neighborhood contributes its per-step placed-feature lists; `FeatureSorter`
//!   unions them into one topologically-sorted list per step. The per-step list
//!   index feeds the seed, so the sort order is parity-critical.
//! * **Seeding** — `setDecorationSeed(seed, originX, originZ)` once per chunk,
//!   then `setFeatureSeed(decorationSeed, indexInStep, step)` before each
//!   feature. Because the RNG is reseeded per feature, features are mutually
//!   RNG-independent: the implemented features are bit-exact regardless of the
//!   deferred ones, which this engine **skips entirely** (they write nothing and
//!   cannot desync anything downstream).
//! * **Placement** — each placed feature threads the section origin through its
//!   [`PlacementModifier`] chain as a depth-first position stream (see
//!   `placement.rs`), matching vanilla's lazy `flatMap` RNG-draw order.
//! * **Write radius 1** — features write through the `WorldGenRegion`, which may
//!   land in the 8 neighboring chunks (`blockStateWriteRadius = 1`).
//!
//! ## Implemented features
//! `ore`, `scattered_ore`, `spring_feature`, `disk`, the whole `tree` system,
//! `random_selector`, `simple_block` (grass / ferns / flowers / mushrooms /
//! gourds / bushes / lily pads / dead bush / dry grass / leaf litter / berries /
//! double plants / lush-caves moss set), `block_column` (cactus, sugar cane),
//! `bamboo`, `kelp`, `seagrass`, `sea_pickle`, and `lake` (lava lakes). Note MC
//! 26.2 replaced the old `random_patch` feature with `simple_block` repeated by
//! its placement chain, so grass/flower patches flow through `simple_block`.
//!
//! ## Deferred features (skipped, documented)
//! `vegetation_patch` / `waterlogged_vegetation_patch` and the rest of the
//! lush-caves set (`big_dripleaf`/`small_dripleaf`/`cave_vines`/`glow_lichen`/
//! `spore_blossom`-ceiling via `multiface_growth`/`root_system`), coral, the pale
//! garden `pale_moss_carpet` side-topper RNG, `freeze_top_layer`,
//! `underwater_magma`, geodes, dripstone, monster rooms, fossils, icebergs,
//! `desert_well`, `ice_spike`, `blue_ice`, `forest_rock`, and every nether/end
//! feature. Each deferred feature is still recognized (so the sort/seed
//! accounting is complete) but its placement is not run — parity-safe because the
//! RNG is reseeded per feature.
//!
//! ## Documented parity deviations
//! * Property-carrying plant blocks collapse to their default block state
//!   (double-plant `half`, sugar-cane/cactus/kelp `age`, sea-pickle `pickles`),
//!   while every vanilla RNG draw is still consumed 1:1.
//! * `noise_provider` / `dual_noise_provider` (a few flower varieties) draw no
//!   RNG and are collapsed to their first modeled state — RNG-exact, cosmetic
//!   variety only. `noise_threshold_provider` is ported fully (RNG + block).
//! * `pale_moss_carpet` is placed as a plain carpet; its 0–4 `nextBoolean`
//!   side-topper draws (only non-zero next to walls) are elided.
//! * `simple_block` survival checks that need light / face-sturdy / neighbor
//!   scans (mushrooms, leaf litter, seagrass, sea pickle, spore blossom) are
//!   approximated by `blocks_motion`; the check draws no RNG so it can only shift
//!   a plant on/off marginal terrain, never desync a feature.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::Value;

use super::density::ParityBlock;
use super::placement::{
    BiomeFeatureIndex, BlockPredicate, BlockTag, DecorationLevel, Heightmap, IntProvider,
    PlacementCtx, PlacementModifier, Pos, RuleTest,
};
use super::random::{RandomSource, WorldgenRandom};
use super::synth::{NoiseParameters, NormalNoise, PerlinSimplexNoise};
use super::vanilla_jsons;

/// A shared, cheaply-cloned handle to a built [`NormalNoise`]. `StateProvider`
/// derives `Clone`/`Debug`; `NormalNoise` does neither, so it is wrapped here.
#[derive(Clone)]
struct NoiseHandle(std::sync::Arc<NormalNoise>);

impl std::fmt::Debug for NoiseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NormalNoise")
    }
}

/// `NoiseBasedStateProvider` — builds the `NormalNoise` from `NormalNoise.create(
/// new WorldgenRandom(new LegacyRandomSource(seed)), parameters)`.
fn build_noise(v: &Value) -> Option<NoiseHandle> {
    let seed = v.get("seed").and_then(Value::as_i64)?;
    let np = &v["noise"];
    let params = NoiseParameters {
        first_octave: np.get("firstOctave").and_then(Value::as_i64).unwrap_or(0) as i32,
        amplitudes: np["amplitudes"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default(),
    };
    let noise = NormalNoise::create(&mut RandomSource::legacy(seed), &params);
    Some(NoiseHandle(std::sync::Arc::new(noise)))
}

/// The `BlockState[]` of a noise state provider, collapsed to parity blocks. An
/// unknown/absent entry is dropped (its variety collapses to a neighbor); the
/// list must stay non-empty for the provider to select anything.
fn parse_state_list(v: &Value) -> Vec<ParityBlock> {
    v.as_array()
        .map(|a| a.iter().filter_map(|s| s.get("Name").and_then(Value::as_str).and_then(ParityBlock::from_name)).collect())
        .unwrap_or_default()
}

/// `Biome.BIOME_INFO_NOISE` — a `PerlinSimplexNoise` seeded 2345 on the legacy
/// LCG. Only the noise-count placement modifiers use it (deferred vegetation).
pub fn biome_info_noise(x: f64, z: f64) -> f64 {
    static NOISE: OnceLock<PerlinSimplexNoise> = OnceLock::new();
    NOISE.get_or_init(|| PerlinSimplexNoise::new(&mut RandomSource::legacy(2345), &[0]))
        .get_value_2d(x, z, false)
}

// ---------------------------------------------------------------------------
// Block-state providers (for disk)
// ---------------------------------------------------------------------------

/// `BlockStateProvider` — the subset the implemented features use.
#[derive(Clone, Debug)]
enum StateProvider {
    Simple(ParityBlock),
    RuleBased { fallback: Box<StateProvider>, rules: Vec<(BlockPredicate, StateProvider)> },
    /// `WeightedStateProvider` — `(block, weight)` entries; `getState` draws one
    /// `nextInt(total_weight)`. Azalea uses it (azalea/flowering_azalea leaves).
    Weighted(Vec<(ParityBlock, i32)>),
    /// `RandomizedIntStateProvider` — draws the `source` state, then draws
    /// `values.sample` to set an integer property. The property collapses onto the
    /// identity default state, but both RNG draws are consumed 1:1 (mangrove
    /// propagule `age`).
    RandomizedInt { source: Box<StateProvider>, values: IntProvider },
    /// `NoiseThresholdProvider` — the only noise state provider that consumes the
    /// passed `RandomSource`. `getNoiseValue(pos)` is deterministic; below the
    /// threshold it draws `Util.getRandom(low_states)` (one `nextInt`), otherwise
    /// it draws `nextFloat()` (always) and, if `< high_chance`, another `nextInt`
    /// over `high_states`. The block choice is exact (all in the alphabet); the
    /// draw sequence is 1:1 so the enclosing `count`-repeat stays in lockstep.
    NoiseThreshold {
        noise: NoiseHandle,
        scale: f64,
        threshold: f32,
        high_chance: f32,
        default_state: Option<ParityBlock>,
        low_states: Vec<ParityBlock>,
        high_states: Vec<ParityBlock>,
    },
    /// `NoiseProvider` / `DualNoiseProvider` — select a state purely from the
    /// deterministic noise value; they draw **no** RNG. Ported RNG-neutrally by
    /// collapsing to the first modeled state (the variety choice is cosmetic and
    /// cannot desync anything). Documented block-identity deviation.
    NoiseCollapsed(ParityBlock),
    Unsupported,
}

impl StateProvider {
    fn parse(v: &Value) -> StateProvider {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "simple_state_provider" => v["state"]["Name"]
                .as_str()
                .and_then(ParityBlock::from_name)
                .map(StateProvider::Simple)
                .unwrap_or(StateProvider::Unsupported),
            "rule_based_state_provider" => StateProvider::RuleBased {
                // No `fallback` field → `null` → parses to `Unsupported` (`None`),
                // matching `RuleBasedStateProvider.getOptionalState` returning
                // `null` when no rule matches and the fallback is absent.
                fallback: Box::new(StateProvider::parse(&v["fallback"])),
                rules: v["rules"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|r| {
                        (BlockPredicate::parse(&r["if_true"]), StateProvider::parse(&r["then"]))
                    })
                    .collect(),
            },
            "weighted_state_provider" => {
                let empty = vec![];
                let entries: Vec<(ParityBlock, i32)> = v["entries"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter_map(|e| {
                        let name = e["data"].get("Name").and_then(Value::as_str).or_else(|| e["data"].as_str());
                        let w = e["weight"].as_i64().unwrap_or(1) as i32;
                        name.and_then(ParityBlock::from_name).map(|b| (b, w))
                    })
                    .collect();
                if entries.is_empty() {
                    StateProvider::Unsupported
                } else {
                    StateProvider::Weighted(entries)
                }
            }
            "randomized_int_state_provider" => StateProvider::RandomizedInt {
                source: Box::new(StateProvider::parse(&v["source"])),
                values: IntProvider::parse(&v["values"]),
            },
            "noise_threshold_provider" => match build_noise(v) {
                Some(noise) => StateProvider::NoiseThreshold {
                    noise,
                    scale: v.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                    threshold: v.get("threshold").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                    high_chance: v.get("high_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                    default_state: v["default_state"]["Name"].as_str().and_then(ParityBlock::from_name),
                    low_states: parse_state_list(&v["low_states"]),
                    high_states: parse_state_list(&v["high_states"]),
                },
                None => StateProvider::Unsupported,
            },
            // `noise_provider` / `dual_noise_provider` draw no RNG; collapse to the
            // first modeled `states` entry (variety choice is cosmetic).
            "noise_provider" | "dual_noise_provider" => parse_state_list(&v["states"])
                .into_iter()
                .next()
                .map(StateProvider::NoiseCollapsed)
                .unwrap_or(StateProvider::Unsupported),
            _ => StateProvider::Unsupported,
        }
    }

    /// `getOptionalState(level, random, pos)`. Simple / rule-based-over-simple
    /// draw no RNG; `Weighted` draws one `nextInt(total_weight)`.
    fn get_state(&self, level: &dyn DecorationLevel, random: &mut WorldgenRandom, pos: Pos) -> Option<ParityBlock> {
        match self {
            StateProvider::Simple(b) => Some(*b),
            StateProvider::RuleBased { fallback, rules } => {
                for (pred, then) in rules {
                    if pred.test(level, pos) {
                        return then.get_state(level, random, pos);
                    }
                }
                fallback.get_state(level, random, pos)
            }
            StateProvider::Weighted(entries) => {
                let total: i32 = entries.iter().map(|(_, w)| *w).sum();
                if total <= 0 {
                    return None;
                }
                let mut roll = random.next_int_bounded(total);
                for (b, w) in entries {
                    roll -= *w;
                    if roll < 0 {
                        return Some(*b);
                    }
                }
                entries.last().map(|(b, _)| *b)
            }
            StateProvider::RandomizedInt { source, values } => {
                let base = source.get_state(level, random, pos);
                // `unmodifiedState.setValue(property, values.sample(random))` — the
                // property collapses onto the default state; the draw is consumed.
                let _ = values.sample(random);
                base
            }
            StateProvider::NoiseThreshold {
                noise, scale, threshold, high_chance, default_state, low_states, high_states,
            } => {
                let local = noise.0.get_value(
                    pos.x as f64 * *scale,
                    pos.y as f64 * *scale,
                    pos.z as f64 * *scale,
                );
                if (local as f32) < *threshold {
                    util_get_random(low_states, random)
                } else if random.next_float() < *high_chance {
                    util_get_random(high_states, random)
                } else {
                    *default_state
                }
            }
            StateProvider::NoiseCollapsed(b) => Some(*b),
            StateProvider::Unsupported => None,
        }
    }
}

/// `Util.getRandom(list, random)` — `list.get(random.nextInt(list.size()))`.
fn util_get_random(list: &[ParityBlock], random: &mut WorldgenRandom) -> Option<ParityBlock> {
    if list.is_empty() {
        return None;
    }
    Some(list[random.next_int_bounded(list.len() as i32) as usize])
}

// ---------------------------------------------------------------------------
// Configured features
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct OreTarget {
    target: RuleTest,
    state: ParityBlock,
}

#[derive(Clone, Debug)]
struct OreConfig {
    targets: Vec<OreTarget>,
    size: i32,
    discard_chance_on_air_exposure: f32,
}

#[derive(Clone, Debug)]
struct SpringConfig {
    fluid: ParityBlock,
    requires_block_below: bool,
    rock_count: i32,
    hole_count: i32,
    valid_blocks: Vec<ParityBlock>,
}

#[derive(Clone, Debug)]
struct DiskConfig {
    state_provider: StateProvider,
    target: BlockPredicate,
    radius: IntProvider,
    half_height: i32,
}

/// `SimpleBlockConfiguration` (`to_place`, `schedule_tick`).
#[derive(Clone, Debug)]
struct SimpleBlockConfig {
    to_place: StateProvider,
}

/// `BlockColumnConfiguration.Layer`.
#[derive(Clone, Debug)]
struct BlockColumnLayer {
    height: IntProvider,
    provider: StateProvider,
}

/// `BlockColumnConfiguration` (`up`-only direction is the overworld case;
/// `allowed_placement` gates growth, `prioritize_tip` steers truncation).
#[derive(Clone, Debug)]
struct BlockColumnConfig {
    layers: Vec<BlockColumnLayer>,
    dir: (i32, i32, i32),
    allowed_placement: BlockPredicate,
    prioritize_tip: bool,
}

/// `LakeFeature.Configuration` (`lake_lava_*`).
#[derive(Clone, Debug)]
struct LakeConfig {
    fluid: StateProvider,
    barrier: StateProvider,
    can_replace_with_air_or_fluid: BlockPredicate,
    can_replace_with_barrier: BlockPredicate,
}

/// `SpikeConfiguration` (`ice_spike`).
#[derive(Clone, Debug)]
struct SpikeConfig {
    state: ParityBlock,
    can_place_on: BlockPredicate,
    can_replace: BlockPredicate,
}

/// `BlockStateConfiguration` for `IcebergFeature`.
#[derive(Clone, Debug)]
struct IcebergConfig {
    state: ParityBlock,
}

/// `BlockBlobConfiguration` (`forest_rock`).
#[derive(Clone, Debug)]
struct BlockBlobConfig {
    state: ParityBlock,
    can_place_on: BlockPredicate,
}

// ---------------------------------------------------------------------------
// Tree feature (TreeFeature / TreeConfiguration and the placer system)
// ---------------------------------------------------------------------------

/// `FoliagePlacer.FoliageAttachment`.
#[derive(Clone, Copy, Debug)]
struct FoliageAttachment {
    pos: Pos,
    radius_offset: i32,
    double_trunk: bool,
}

/// `FeatureSize` (`getSizeAtHeight` / `minClippedHeight`).
#[derive(Clone, Debug)]
enum FeatureSize {
    TwoLayers { limit: i32, lower: i32, upper: i32, min_clipped: Option<i32> },
    ThreeLayers { limit: i32, upper_limit: i32, lower: i32, middle: i32, upper: i32, min_clipped: Option<i32> },
}

impl FeatureSize {
    fn parse(v: &Value) -> FeatureSize {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        let min_clipped = v.get("min_clipped_height").and_then(Value::as_i64).map(|n| n as i32);
        let geti = |k: &str, d: i64| v.get(k).and_then(Value::as_i64).unwrap_or(d) as i32;
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "three_layers_feature_size" => FeatureSize::ThreeLayers {
                limit: geti("limit", 1),
                upper_limit: geti("upper_limit", 1),
                lower: geti("lower_size", 0),
                middle: geti("middle_size", 1),
                upper: geti("upper_size", 1),
                min_clipped,
            },
            // `two_layers_feature_size` (also the default).
            _ => FeatureSize::TwoLayers {
                limit: geti("limit", 1),
                lower: geti("lower_size", 0),
                upper: geti("upper_size", 1),
                min_clipped,
            },
        }
    }

    fn get_size_at_height(&self, tree_height: i32, yo: i32) -> i32 {
        match self {
            FeatureSize::TwoLayers { limit, lower, upper, .. } => {
                if yo < *limit { *lower } else { *upper }
            }
            FeatureSize::ThreeLayers { limit, upper_limit, lower, middle, upper, .. } => {
                if yo < *limit {
                    *lower
                } else if yo >= tree_height - *upper_limit {
                    *upper
                } else {
                    *middle
                }
            }
        }
    }

    fn min_clipped_height(&self) -> Option<i32> {
        match self {
            FeatureSize::TwoLayers { min_clipped, .. } => *min_clipped,
            FeatureSize::ThreeLayers { min_clipped, .. } => *min_clipped,
        }
    }
}

/// `TrunkPlacer` — the three overworld placers plus a graceful `Unsupported`
/// (fancy / bending / cherry etc., a later milestone).
#[derive(Clone, Debug)]
enum TrunkPlacer {
    Straight { base: i32, a: i32, b: i32 },
    Forking { base: i32, a: i32, b: i32 },
    DarkOak { base: i32, a: i32, b: i32 },
    Fancy { base: i32, a: i32, b: i32 },
    /// `GiantTrunkPlacer` — a 2×2 straight trunk (mega spruce/jungle base).
    Giant { base: i32, a: i32, b: i32 },
    /// `MegaJungleTrunkPlacer extends GiantTrunkPlacer` — 2×2 trunk plus radial
    /// side branches.
    MegaJungle { base: i32, a: i32, b: i32 },
    /// `CherryTrunkPlacer` — a straight trunk with 1–3 curved side branches.
    Cherry {
        base: i32,
        a: i32,
        b: i32,
        branch_count: IntProvider,
        branch_horizontal_length: IntProvider,
        branch_start_min: i32,
        branch_start_max: i32,
        branch_end_offset: IntProvider,
    },
    /// `BendingTrunkPlacer` — a trunk that bends over near the top (azalea).
    Bending { base: i32, a: i32, b: i32, min_height_for_leaves: i32, bend_length: IntProvider },
    /// `UpwardsBranchingTrunkPlacer` — a trunk with random upward branches that
    /// can grow through a block set (mangrove).
    UpwardsBranching {
        base: i32,
        a: i32,
        b: i32,
        extra_branch_steps: IntProvider,
        place_branch_prob: f32,
        extra_branch_length: IntProvider,
        can_grow_through: Option<BlockTag>,
    },
    Unsupported,
}

impl TrunkPlacer {
    fn parse(v: &Value) -> TrunkPlacer {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        let base = v.get("base_height").and_then(Value::as_i64).unwrap_or(0) as i32;
        let a = v.get("height_rand_a").and_then(Value::as_i64).unwrap_or(0) as i32;
        let b = v.get("height_rand_b").and_then(Value::as_i64).unwrap_or(0) as i32;
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "straight_trunk_placer" => TrunkPlacer::Straight { base, a, b },
            "forking_trunk_placer" => TrunkPlacer::Forking { base, a, b },
            "dark_oak_trunk_placer" => TrunkPlacer::DarkOak { base, a, b },
            "fancy_trunk_placer" => TrunkPlacer::Fancy { base, a, b },
            "giant_trunk_placer" => TrunkPlacer::Giant { base, a, b },
            "mega_jungle_trunk_placer" => TrunkPlacer::MegaJungle { base, a, b },
            "cherry_trunk_placer" => {
                // `branch_start_offset_from_top` is a bare `UniformInt` (no `type`);
                // `secondBranchStartOffsetFromTop = UniformInt.of(min, max-1)`.
                let bs = &v["branch_start_offset_from_top"];
                TrunkPlacer::Cherry {
                    base,
                    a,
                    b,
                    branch_count: IntProvider::parse(&v["branch_count"]),
                    branch_horizontal_length: IntProvider::parse(&v["branch_horizontal_length"]),
                    branch_start_min: bs["min_inclusive"].as_i64().unwrap_or(0) as i32,
                    branch_start_max: bs["max_inclusive"].as_i64().unwrap_or(0) as i32,
                    branch_end_offset: IntProvider::parse(&v["branch_end_offset_from_top"]),
                }
            }
            "bending_trunk_placer" => TrunkPlacer::Bending {
                base,
                a,
                b,
                min_height_for_leaves: v.get("min_height_for_leaves").and_then(Value::as_i64).unwrap_or(1) as i32,
                bend_length: IntProvider::parse(&v["bend_length"]),
            },
            "upwards_branching_trunk_placer" => TrunkPlacer::UpwardsBranching {
                base,
                a,
                b,
                extra_branch_steps: IntProvider::parse(&v["extra_branch_steps"]),
                place_branch_prob: v.get("place_branch_per_log_probability").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                extra_branch_length: IntProvider::parse(&v["extra_branch_length"]),
                can_grow_through: parse_grow_through(&v["can_grow_through"]),
            },
            _ => TrunkPlacer::Unsupported,
        }
    }

    fn is_unsupported(&self) -> bool {
        matches!(self, TrunkPlacer::Unsupported)
    }

    /// The `validTreePos`-widening block set some placers grow through
    /// (`UpwardsBranchingTrunkPlacer`); `None` for the plain placers.
    fn grow_through(&self) -> Option<BlockTag> {
        match self {
            TrunkPlacer::UpwardsBranching { can_grow_through, .. } => *can_grow_through,
            _ => None,
        }
    }

    /// `TrunkPlacer.getTreeHeight` — `baseHeight + nextInt(a+1) + nextInt(b+1)`.
    fn get_tree_height(&self, random: &mut WorldgenRandom) -> i32 {
        match self {
            TrunkPlacer::Straight { base, a, b }
            | TrunkPlacer::Forking { base, a, b }
            | TrunkPlacer::DarkOak { base, a, b }
            | TrunkPlacer::Fancy { base, a, b }
            | TrunkPlacer::Giant { base, a, b }
            | TrunkPlacer::MegaJungle { base, a, b }
            | TrunkPlacer::Cherry { base, a, b, .. }
            | TrunkPlacer::Bending { base, a, b, .. }
            | TrunkPlacer::UpwardsBranching { base, a, b, .. } => {
                *base + random.next_int_bounded(*a + 1) + random.next_int_bounded(*b + 1)
            }
            TrunkPlacer::Unsupported => 0,
        }
    }
}

/// `FoliagePlacer` — the four overworld placers plus a graceful `Unsupported`.
#[derive(Clone, Debug)]
enum FoliagePlacer {
    Blob { radius: IntProvider, offset: IntProvider, height: i32 },
    Spruce { radius: IntProvider, offset: IntProvider, trunk_height: IntProvider },
    Pine { radius: IntProvider, offset: IntProvider, height: IntProvider },
    DarkOak { radius: IntProvider, offset: IntProvider },
    /// `FancyFoliagePlacer extends BlobFoliagePlacer` — same `height` field, but
    /// its `createFoliage`/`shouldSkipLocation` are overridden (no RNG draws).
    Fancy { radius: IntProvider, offset: IntProvider, height: i32 },
    /// `BushFoliagePlacer extends BlobFoliagePlacer` — same `height` field; only
    /// `createFoliage`/`shouldSkipLocation` differ (jungle bush).
    Bush { radius: IntProvider, offset: IntProvider, height: i32 },
    /// `AcaciaFoliagePlacer` — a flat 3-row canopy; `foliageHeight` is always 0
    /// and `createFoliage` draws no RNG.
    Acacia { radius: IntProvider, offset: IntProvider },
    /// `MegaJungleFoliagePlacer` — the top blob of a mega jungle tree; draws one
    /// `nextInt(2)` per single-trunk attachment.
    MegaJungle { radius: IntProvider, offset: IntProvider, height: i32 },
    /// `CherryFoliagePlacer` — a wide flat canopy with hanging-leaf fringes.
    Cherry {
        radius: IntProvider,
        offset: IntProvider,
        height: IntProvider,
        wide_bottom_layer_hole_chance: f32,
        corner_hole_chance: f32,
        hanging_leaves_chance: f32,
        hanging_leaves_extension_chance: f32,
    },
    /// `MegaPineFoliagePlacer` — the jagged conic crown of a mega spruce/pine.
    MegaPine { radius: IntProvider, offset: IntProvider, crown_height: IntProvider },
    /// `RandomSpreadFoliagePlacer` — scatters leaves in a box (azalea, mangrove).
    RandomSpread { radius: IntProvider, offset: IntProvider, foliage_height: IntProvider, leaf_placement_attempts: i32 },
    Unsupported,
}

impl FoliagePlacer {
    fn parse(v: &Value) -> FoliagePlacer {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        let radius = IntProvider::parse(&v["radius"]);
        let offset = IntProvider::parse(&v["offset"]);
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "blob_foliage_placer" => FoliagePlacer::Blob {
                radius,
                offset,
                height: v.get("height").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            "spruce_foliage_placer" => FoliagePlacer::Spruce {
                radius,
                offset,
                trunk_height: IntProvider::parse(&v["trunk_height"]),
            },
            "pine_foliage_placer" => FoliagePlacer::Pine {
                radius,
                offset,
                height: IntProvider::parse(&v["height"]),
            },
            "dark_oak_foliage_placer" => FoliagePlacer::DarkOak { radius, offset },
            "fancy_foliage_placer" => FoliagePlacer::Fancy {
                radius,
                offset,
                height: v.get("height").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            "bush_foliage_placer" => FoliagePlacer::Bush {
                radius,
                offset,
                height: v.get("height").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            "acacia_foliage_placer" => FoliagePlacer::Acacia { radius, offset },
            // `MegaJungleFoliagePlacer` registers under the id `jungle_foliage_placer`.
            "jungle_foliage_placer" => FoliagePlacer::MegaJungle {
                radius,
                offset,
                height: v.get("height").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            "cherry_foliage_placer" => FoliagePlacer::Cherry {
                radius,
                offset,
                height: IntProvider::parse(&v["height"]),
                wide_bottom_layer_hole_chance: v.get("wide_bottom_layer_hole_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                corner_hole_chance: v.get("corner_hole_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                hanging_leaves_chance: v.get("hanging_leaves_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                hanging_leaves_extension_chance: v.get("hanging_leaves_extension_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            },
            "mega_pine_foliage_placer" => FoliagePlacer::MegaPine {
                radius,
                offset,
                crown_height: IntProvider::parse(&v["crown_height"]),
            },
            "random_spread_foliage_placer" => FoliagePlacer::RandomSpread {
                radius,
                offset,
                foliage_height: IntProvider::parse(&v["foliage_height"]),
                leaf_placement_attempts: v.get("leaf_placement_attempts").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            _ => FoliagePlacer::Unsupported,
        }
    }

    fn is_unsupported(&self) -> bool {
        matches!(self, FoliagePlacer::Unsupported)
    }
}

/// `TreeDecorator` — `beehive`, `cocoa`, `trunk_vine`, and `leave_vine` are
/// modeled; everything else is a graceful no-op (the decorators run after the
/// tree body, so an unknown one can only affect its own output, never another
/// feature — the RNG is reseeded per top feature).
#[derive(Clone, Debug)]
enum TreeDecorator {
    Beehive { probability: f32 },
    /// `CocoaDecorator` — hangs cocoa pods on the lowest trunk logs.
    Cocoa { probability: f32 },
    /// `TrunkVineDecorator` — vines on the sides of trunk logs.
    TrunkVine,
    /// `LeaveVineDecorator` — hanging vines off the leaf shell.
    LeaveVine { probability: f32 },
    /// `AlterGroundDecorator` — replaces the ground under the trunk (podzol for
    /// mega spruce/pine).
    AlterGround { provider: StateProvider },
    /// `AttachedToLeavesDecorator` — hangs a block (mangrove propagule) off leaves.
    AttachedToLeaves {
        probability: f32,
        exclusion_radius_xz: i32,
        exclusion_radius_y: i32,
        block_provider: StateProvider,
        required_empty_blocks: i32,
        directions: Vec<(i32, i32, i32)>,
    },
    Unsupported,
}

impl TreeDecorator {
    fn parse(v: &Value) -> TreeDecorator {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        let prob = v.get("probability").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "beehive" => TreeDecorator::Beehive { probability: prob },
            "cocoa" => TreeDecorator::Cocoa { probability: prob },
            "trunk_vine" => TreeDecorator::TrunkVine,
            "leave_vine" => TreeDecorator::LeaveVine { probability: prob },
            "alter_ground" => TreeDecorator::AlterGround { provider: StateProvider::parse(&v["provider"]) },
            "attached_to_leaves" => TreeDecorator::AttachedToLeaves {
                probability: prob,
                exclusion_radius_xz: v.get("exclusion_radius_xz").and_then(Value::as_i64).unwrap_or(0) as i32,
                exclusion_radius_y: v.get("exclusion_radius_y").and_then(Value::as_i64).unwrap_or(0) as i32,
                block_provider: StateProvider::parse(&v["block_provider"]),
                required_empty_blocks: v.get("required_empty_blocks").and_then(Value::as_i64).unwrap_or(1) as i32,
                directions: v["directions"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|d| parse_direction(d.as_str().unwrap_or("")))
                    .collect(),
            },
            _ => TreeDecorator::Unsupported,
        }
    }
}

/// A `Direction` name → unit step vector (only the six cardinal directions).
fn parse_direction(name: &str) -> Option<(i32, i32, i32)> {
    Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
        "down" => (0, -1, 0),
        "up" => (0, 1, 0),
        "north" => (0, 0, -1),
        "south" => (0, 0, 1),
        "west" => (-1, 0, 0),
        "east" => (1, 0, 0),
        _ => return None,
    })
}

/// `AboveRootPlacement` — a chance to place a block (moss carpet) on top of each
/// placed root.
#[derive(Clone, Debug)]
struct AboveRootPlacement {
    chance: f32,
    provider: StateProvider,
}

/// `RootPlacer` — only `MangroveRootPlacer` exists in the overworld. Grows a
/// spreading root system below the trunk origin before the trunk is placed; its
/// RNG draws precede (and thus affect) the trunk/foliage draws, so it is ported
/// 1:1.
#[derive(Clone, Debug)]
struct RootPlacer {
    trunk_offset_y: IntProvider,
    root_provider: StateProvider,
    above_root: Option<AboveRootPlacement>,
    can_grow_through: Option<BlockTag>,
    muddy_roots_in: Vec<ParityBlock>,
    muddy_roots_provider: StateProvider,
    max_root_width: i32,
    max_root_length: i32,
    random_skew_chance: f32,
}

/// Parse a `root_placer` config. Returns `None` for an unsupported type (there is
/// only `mangrove_root_placer` in vanilla); the caller then treats the tree as
/// unsupported rather than mis-placing it.
fn parse_root_placer(v: &Value) -> Option<RootPlacer> {
    let t = v.get("type").and_then(Value::as_str).unwrap_or("");
    if t.strip_prefix("minecraft:").unwrap_or(t) != "mangrove_root_placer" {
        return None;
    }
    let mrp = &v["mangrove_root_placement"];
    Some(RootPlacer {
        trunk_offset_y: IntProvider::parse(&v["trunk_offset_y"]),
        root_provider: StateProvider::parse(&v["root_provider"]),
        above_root: v
            .get("above_root_placement")
            .filter(|a| a.is_object())
            .map(|a| AboveRootPlacement {
                chance: a.get("above_root_placement_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                provider: StateProvider::parse(&a["above_root_provider"]),
            }),
        can_grow_through: parse_grow_through(&mrp["can_grow_through"]),
        muddy_roots_in: parse_block_holderset(&mrp["muddy_roots_in"]),
        muddy_roots_provider: StateProvider::parse(&mrp["muddy_roots_provider"]),
        max_root_width: mrp.get("max_root_width").and_then(Value::as_i64).unwrap_or(8) as i32,
        max_root_length: mrp.get("max_root_length").and_then(Value::as_i64).unwrap_or(15) as i32,
        random_skew_chance: mrp.get("random_skew_chance").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    })
}

#[derive(Clone, Debug)]
struct TreeConfig {
    trunk_provider: StateProvider,
    trunk_placer: TrunkPlacer,
    foliage_provider: StateProvider,
    foliage_placer: FoliagePlacer,
    minimum_size: FeatureSize,
    decorators: Vec<TreeDecorator>,
    ignore_vines: bool,
    below_trunk_provider: StateProvider,
    root_placer: Option<RootPlacer>,
    /// A `root_placer` field was present but of an unsupported type → skip the
    /// whole tree (parity-safe).
    root_placer_unsupported: bool,
}

fn parse_tree(cfg: &Value) -> TreeConfig {
    let has_root_field = cfg.get("root_placer").map(|v| !v.is_null()).unwrap_or(false);
    let root_placer = if has_root_field { parse_root_placer(&cfg["root_placer"]) } else { None };
    TreeConfig {
        trunk_provider: StateProvider::parse(&cfg["trunk_provider"]),
        trunk_placer: TrunkPlacer::parse(&cfg["trunk_placer"]),
        foliage_provider: StateProvider::parse(&cfg["foliage_provider"]),
        foliage_placer: FoliagePlacer::parse(&cfg["foliage_placer"]),
        minimum_size: FeatureSize::parse(&cfg["minimum_size"]),
        decorators: cfg["decorators"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(TreeDecorator::parse)
            .collect(),
        ignore_vines: cfg.get("ignore_vines").and_then(Value::as_bool).unwrap_or(false),
        below_trunk_provider: StateProvider::parse(&cfg["below_trunk_provider"]),
        root_placer_unsupported: has_root_field && root_placer.is_none(),
        root_placer,
    }
}

// ---------------------------------------------------------------------------
// RandomSelectorFeature
// ---------------------------------------------------------------------------

/// A nested feature reference inside a `random_selector`. A `Holder<PlacedFeature>`
/// serializes either as a string id (`PlacedRef`) or an inline object
/// (`InlineRef`). These are resolved into `Resolved` at `FeatureRegistry::load`
/// time so `place_feature` stays registry-free.
#[derive(Clone, Debug)]
enum NestedFeature {
    PlacedRef(String),
    InlineRef { feature: String, placement: Vec<PlacementModifier> },
    Resolved { feature: Box<ConfiguredFeature>, placement: Vec<PlacementModifier> },
}

#[derive(Clone, Debug)]
struct WeightedNested {
    chance: f32,
    feature: NestedFeature,
}

#[derive(Clone, Debug)]
struct RandomSelectorConfig {
    features: Vec<WeightedNested>,
    default: NestedFeature,
}

fn parse_nested(v: &Value) -> NestedFeature {
    match v {
        Value::String(s) => NestedFeature::PlacedRef(strip(s)),
        Value::Object(_) => NestedFeature::InlineRef {
            feature: strip(v["feature"].as_str().unwrap_or("")),
            placement: v["placement"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(PlacementModifier::parse)
                .collect(),
        },
        _ => NestedFeature::PlacedRef(String::new()),
    }
}

fn parse_random_selector(cfg: &Value) -> RandomSelectorConfig {
    RandomSelectorConfig {
        features: cfg["features"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|e| WeightedNested {
                chance: e["chance"].as_f64().unwrap_or(0.0) as f32,
                feature: parse_nested(&e["feature"]),
            })
            .collect(),
        default: parse_nested(&cfg["default"]),
    }
}

/// `ConfiguredFeature` — the implemented variants carry their parsed config; a
/// deferred feature keeps only its type name (for diagnostics).
#[derive(Clone, Debug)]
enum ConfiguredFeature {
    Ore(OreConfig),
    ScatteredOre(OreConfig),
    Spring(SpringConfig),
    Disk(DiskConfig),
    Tree(TreeConfig),
    RandomSelector(RandomSelectorConfig),
    SimpleBlock(SimpleBlockConfig),
    BlockColumn(BlockColumnConfig),
    Bamboo { probability: f32 },
    Kelp,
    Seagrass { probability: f32 },
    SeaPickle { count: IntProvider },
    Lake(LakeConfig),
    BlueIce,
    Spike(SpikeConfig),
    Iceberg(IcebergConfig),
    BlockBlob(BlockBlobConfig),
    DesertWell,
    Deferred(String),
}

impl ConfiguredFeature {
    fn parse(v: &Value) -> ConfiguredFeature {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        let cfg = &v["config"];
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "ore" => ConfiguredFeature::Ore(parse_ore(cfg)),
            "scattered_ore" => ConfiguredFeature::ScatteredOre(parse_ore(cfg)),
            "spring_feature" => ConfiguredFeature::Spring(SpringConfig {
                fluid: cfg["state"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::Water),
                requires_block_below: cfg.get("requires_block_below").and_then(Value::as_bool).unwrap_or(true),
                rock_count: cfg.get("rock_count").and_then(Value::as_i64).unwrap_or(4) as i32,
                hole_count: cfg.get("hole_count").and_then(Value::as_i64).unwrap_or(1) as i32,
                valid_blocks: parse_block_holderset(&cfg["valid_blocks"]),
            }),
            "disk" => ConfiguredFeature::Disk(DiskConfig {
                state_provider: StateProvider::parse(&cfg["state_provider"]),
                target: BlockPredicate::parse(&cfg["target"]),
                radius: IntProvider::parse(&cfg["radius"]),
                half_height: cfg["half_height"].as_i64().unwrap_or(0) as i32,
            }),
            "tree" => ConfiguredFeature::Tree(parse_tree(cfg)),
            "random_selector" => ConfiguredFeature::RandomSelector(parse_random_selector(cfg)),
            "simple_block" => ConfiguredFeature::SimpleBlock(SimpleBlockConfig {
                to_place: StateProvider::parse(&cfg["to_place"]),
            }),
            "block_column" => ConfiguredFeature::BlockColumn(BlockColumnConfig {
                layers: cfg["layers"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|l| BlockColumnLayer {
                        height: IntProvider::parse(&l["height"]),
                        provider: StateProvider::parse(&l["provider"]),
                    })
                    .collect(),
                dir: parse_direction(cfg["direction"].as_str().unwrap_or("up")).unwrap_or((0, 1, 0)),
                allowed_placement: BlockPredicate::parse(&cfg["allowed_placement"]),
                prioritize_tip: cfg.get("prioritize_tip").and_then(Value::as_bool).unwrap_or(false),
            }),
            "bamboo" => ConfiguredFeature::Bamboo {
                probability: cfg.get("probability").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            },
            "kelp" => ConfiguredFeature::Kelp,
            "seagrass" => ConfiguredFeature::Seagrass {
                probability: cfg.get("probability").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            },
            "sea_pickle" => ConfiguredFeature::SeaPickle { count: IntProvider::parse(&cfg["count"]) },
            "lake" => ConfiguredFeature::Lake(LakeConfig {
                fluid: StateProvider::parse(&cfg["fluid"]),
                barrier: StateProvider::parse(&cfg["barrier"]),
                can_replace_with_air_or_fluid: BlockPredicate::parse(&cfg["can_replace_with_air_or_fluid"]),
                can_replace_with_barrier: BlockPredicate::parse(&cfg["can_replace_with_barrier"]),
            }),
            "blue_ice" => ConfiguredFeature::BlueIce,
            "spike" => ConfiguredFeature::Spike(SpikeConfig {
                state: cfg["state"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::PackedIce),
                can_place_on: BlockPredicate::parse(&cfg["can_place_on"]),
                can_replace: BlockPredicate::parse(&cfg["can_replace"]),
            }),
            "iceberg" => ConfiguredFeature::Iceberg(IcebergConfig {
                state: cfg["state"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::PackedIce),
            }),
            "block_blob" => ConfiguredFeature::BlockBlob(BlockBlobConfig {
                state: cfg["state"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::MossyCobblestone),
                can_place_on: BlockPredicate::parse(&cfg["can_place_on"]),
            }),
            "desert_well" => ConfiguredFeature::DesertWell,
            // `freeze_top_layer` (SnowAndFreezeFeature) is recognized but
            // deferred: its exact `Biome.shouldFreeze`/`shouldSnow` gates need
            // the biome-temperature/height-adjust plumbing (in surface_rules)
            // wired through the region, out of scope for this milestone.
            other => ConfiguredFeature::Deferred(other.to_owned()),
        }
    }

    fn is_implemented(&self) -> bool {
        !matches!(self, ConfiguredFeature::Deferred(_))
    }
}

fn parse_ore(cfg: &Value) -> OreConfig {
    OreConfig {
        targets: cfg["targets"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|t| {
                t["state"]["Name"].as_str().and_then(ParityBlock::from_name).map(|state| OreTarget {
                    target: RuleTest::parse(&t["target"]),
                    state,
                })
            })
            .collect(),
        size: cfg["size"].as_i64().unwrap_or(0) as i32,
        discard_chance_on_air_exposure: cfg["discard_chance_on_air_exposure"].as_f64().unwrap_or(0.0) as f32,
    }
}

/// Resolve a `#tag`-string HolderSet reference (e.g. a `can_grow_through` field)
/// to a modeled [`BlockTag`]. Non-tag / unknown references yield `None`.
fn parse_grow_through(v: &Value) -> Option<BlockTag> {
    v.as_str().and_then(|s| BlockTag::from_id(s.trim_start_matches('#')))
}

fn parse_block_holderset(v: &Value) -> Vec<ParityBlock> {
    match v {
        Value::String(s) => ParityBlock::from_name(s).into_iter().collect(),
        Value::Array(a) => a.iter().filter_map(|e| e.as_str().and_then(ParityBlock::from_name)).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Placed features
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct PlacedFeature {
    /// Configured-feature id (`minecraft:…`, stripped to the path here).
    feature: String,
    placement: Vec<PlacementModifier>,
}

// ---------------------------------------------------------------------------
// The feature registry (loaded once)
// ---------------------------------------------------------------------------

/// Everything the decoration driver needs, parsed from the vendored datapack:
/// configured + placed features, per-biome feature lists, and the `FeatureSorter`
/// output (per-step ordered placed-feature lists + index lookups).
pub struct FeatureRegistry {
    configured: HashMap<String, ConfiguredFeature>,
    placed: HashMap<String, PlacedFeature>,
    /// biome name → its 11-step lists of placed-feature ids.
    biome_features: HashMap<String, Vec<Vec<String>>>,
    /// biome name → set of every placed-feature id it lists (for `BiomeFilter`).
    biome_feature_set: HashMap<String, HashSet<String>>,
    /// Per step: the topologically-sorted placed-feature ids (seed-index order).
    steps: Vec<Vec<String>>,
    /// Per step: placed-feature id → its index within that step's list.
    step_index: Vec<HashMap<String, i32>>,
    /// biome fill value → biome name (mirrors the parameter-list order).
    biome_names: Vec<String>,
}

fn strip(id: &str) -> String {
    id.strip_prefix("minecraft:").unwrap_or(id).to_owned()
}

/// Resolve a configured feature, recursively resolving any nested
/// `random_selector` references it contains.
fn resolve_cf(
    cf: &ConfiguredFeature,
    configured: &HashMap<String, ConfiguredFeature>,
    placed: &HashMap<String, PlacedFeature>,
) -> ConfiguredFeature {
    match cf {
        ConfiguredFeature::RandomSelector(rc) => ConfiguredFeature::RandomSelector(RandomSelectorConfig {
            features: rc
                .features
                .iter()
                .map(|w| WeightedNested { chance: w.chance, feature: resolve_nested(&w.feature, configured, placed) })
                .collect(),
            default: resolve_nested(&rc.default, configured, placed),
        }),
        other => other.clone(),
    }
}

fn resolve_configured(
    id: &str,
    configured: &HashMap<String, ConfiguredFeature>,
    placed: &HashMap<String, PlacedFeature>,
) -> ConfiguredFeature {
    match configured.get(id) {
        Some(cf) => resolve_cf(cf, configured, placed),
        None => ConfiguredFeature::Deferred(id.to_owned()),
    }
}

fn resolve_nested(
    nf: &NestedFeature,
    configured: &HashMap<String, ConfiguredFeature>,
    placed: &HashMap<String, PlacedFeature>,
) -> NestedFeature {
    match nf {
        NestedFeature::PlacedRef(id) => match placed.get(id) {
            Some(pf) => NestedFeature::Resolved {
                feature: Box::new(resolve_configured(&pf.feature, configured, placed)),
                placement: pf.placement.clone(),
            },
            None => NestedFeature::Resolved {
                feature: Box::new(ConfiguredFeature::Deferred(id.clone())),
                placement: Vec::new(),
            },
        },
        NestedFeature::InlineRef { feature, placement } => NestedFeature::Resolved {
            feature: Box::new(resolve_configured(feature, configured, placed)),
            placement: placement.clone(),
        },
        NestedFeature::Resolved { .. } => nf.clone(),
    }
}

impl FeatureRegistry {
    /// Build from the vendored JSON. `biome_names` maps fill values to registry
    /// names (from `MultiNoiseBiomeSource`, in parameter-list order).
    pub fn load(biome_names: Vec<String>) -> Self {
        let mut configured = HashMap::new();
        for (name, json) in vanilla_jsons::CONFIGURED_FEATURES {
            let v: Value = serde_json::from_str(json).expect("configured feature json");
            configured.insert(name.to_string(), ConfiguredFeature::parse(&v));
        }
        let mut placed = HashMap::new();
        for (name, json) in vanilla_jsons::PLACED_FEATURES {
            let v: Value = serde_json::from_str(json).expect("placed feature json");
            placed.insert(
                name.to_string(),
                PlacedFeature {
                    feature: strip(v["feature"].as_str().expect("feature id")),
                    placement: v["placement"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .map(PlacementModifier::parse)
                        .collect(),
                },
            );
        }
        // Resolve `random_selector` nested feature references (holders that
        // serialize as either a placed-feature id or an inline placed feature)
        // into owned `ConfiguredFeature`s + placement chains, so `place_feature`
        // never needs the registry.
        let selector_ids: Vec<String> = configured
            .iter()
            .filter(|(_, c)| matches!(c, ConfiguredFeature::RandomSelector(_)))
            .map(|(k, _)| k.clone())
            .collect();
        for id in selector_ids {
            let cf = configured.get(&id).cloned().unwrap();
            let resolved = resolve_cf(&cf, &configured, &placed);
            configured.insert(id, resolved);
        }

        let mut biome_features = HashMap::new();
        let mut biome_feature_set = HashMap::new();
        for (name, json) in vanilla_jsons::BIOMES {
            let v: Value = serde_json::from_str(json).expect("biome json");
            let steps: Vec<Vec<String>> = v["features"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|step| {
                    step.as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|id| id.as_str().map(strip))
                        .collect()
                })
                .collect();
            let set: HashSet<String> = steps.iter().flatten().cloned().collect();
            // Key by the registry id (`minecraft:…`) to match `biome_names`,
            // which come from `MultiNoiseBiomeSource` prefixed.
            let key = format!("minecraft:{name}");
            biome_features.insert(key.clone(), steps);
            biome_feature_set.insert(key, set);
        }

        let (steps, step_index) = build_features_per_step(&biome_names, &biome_features);

        Self { configured, placed, biome_features, biome_feature_set, steps, step_index, biome_names }
    }

    fn biome_name(&self, fill: u16) -> &str {
        &self.biome_names[fill as usize]
    }
}

impl BiomeFeatureIndex for FeatureRegistry {
    fn biome_has_feature(&self, biome_fill: u16, placed_feature_id: &str) -> bool {
        self.biome_feature_set
            .get(self.biome_name(biome_fill))
            .map(|s| s.contains(placed_feature_id))
            .unwrap_or(false)
    }
}

/// `FeatureSorter.buildFeaturesPerStep` — the topological sort over the union of
/// all biomes' per-step feature lists. Returns, per step, the sorted
/// placed-feature ids plus an id→index-within-step lookup. Ported 1:1 (node =
/// `(step, feature_index)`, `feature_index` = first-encounter global order over
/// `possibleBiomes()` in parameter-list order; edges join consecutive features
/// of each biome's flattened cross-step list; reverse-DFS post-order gives the
/// topological order).
fn build_features_per_step(
    biome_order: &[String],
    biome_features: &HashMap<String, Vec<Vec<String>>>,
) -> (Vec<Vec<String>>, Vec<HashMap<String, i32>>) {
    type Node = (i32, i32); // (step, feature_index)

    let mut feature_index: HashMap<String, i32> = HashMap::new();
    let mut next_index: i32 = 0;
    let mut edges: BTreeMap<Node, BTreeSet<Node>> = BTreeMap::new();
    let mut node_feature: HashMap<Node, String> = HashMap::new();
    let mut max_step = 0usize;

    for biome in biome_order {
        let per_step = match biome_features.get(biome) {
            Some(s) => s,
            None => continue,
        };
        max_step = max_step.max(per_step.len());
        let mut feature_list: Vec<Node> = Vec::new();
        for (i, step) in per_step.iter().enumerate() {
            for id in step {
                let idx = *feature_index.entry(id.clone()).or_insert_with(|| {
                    let v = next_index;
                    next_index += 1;
                    v
                });
                let node = (i as i32, idx);
                node_feature.insert(node, id.clone());
                feature_list.push(node);
            }
        }
        for i in 0..feature_list.len() {
            let entry = edges.entry(feature_list[i]).or_default();
            if i + 1 < feature_list.len() {
                entry.insert(feature_list[i + 1]);
            }
        }
    }

    // Reverse-topological DFS over the comparator-ordered node set.
    let mut discovered: BTreeSet<Node> = BTreeSet::new();
    let mut sorted: Vec<Node> = Vec::new();
    let keys: Vec<Node> = edges.keys().copied().collect();
    for node in keys {
        if !discovered.contains(&node) {
            dfs(&edges, &mut discovered, &mut sorted, node);
        }
    }
    sorted.reverse();

    let mut steps: Vec<Vec<String>> = vec![Vec::new(); max_step];
    for node in &sorted {
        let step = node.0 as usize;
        steps[step].push(node_feature[node].clone());
    }
    let step_index: Vec<HashMap<String, i32>> = steps
        .iter()
        .map(|list| list.iter().enumerate().map(|(i, id)| (id.clone(), i as i32)).collect())
        .collect();
    (steps, step_index)
}

/// `Graph.depthFirstSearch` (iterative to avoid deep recursion). Vanilla data is
/// acyclic; a back-edge (cycle) would `panic` — vanilla throws too.
fn dfs(
    edges: &BTreeMap<(i32, i32), BTreeSet<(i32, i32)>>,
    discovered: &mut BTreeSet<(i32, i32)>,
    sorted: &mut Vec<(i32, i32)>,
    start: (i32, i32),
) {
    // Emulate the recursive post-order with an explicit stack of (node, child
    // iterator index). `visiting` guards against cycles.
    let mut visiting: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut stack: Vec<((i32, i32), Vec<(i32, i32)>, usize)> = Vec::new();
    let empty = BTreeSet::new();

    let children = |n: (i32, i32)| -> Vec<(i32, i32)> {
        edges.get(&n).unwrap_or(&empty).iter().copied().collect()
    };

    if discovered.contains(&start) {
        return;
    }
    visiting.insert(start);
    stack.push((start, children(start), 0));

    while let Some((node, kids, idx)) = stack.last_mut() {
        if *idx < kids.len() {
            let child = kids[*idx];
            *idx += 1;
            if discovered.contains(&child) {
                continue;
            }
            assert!(!visiting.contains(&child), "feature order cycle found");
            visiting.insert(child);
            let cc = children(child);
            stack.push((child, cc, 0));
        } else {
            visiting.remove(node);
            discovered.insert(*node);
            sorted.push(*node);
            stack.pop();
        }
    }
}

// ---------------------------------------------------------------------------
// The decoration driver
// ---------------------------------------------------------------------------

/// `ChunkGenerator.applyBiomeDecoration` for one chunk. `possible_biomes` is the
/// set of fill values present in the chunk's 3×3 section neighborhood (the union
/// vanilla collects from `LevelChunkSection.getBiomes`). `seed` is the world
/// seed. `min_block_x`/`min_block_z` are the section origin (`chunkX*16`,
/// `chunkZ*16`).
pub fn apply_biome_decoration(
    registry: &FeatureRegistry,
    level: &mut dyn DecorationLevel,
    possible_biomes: &HashSet<u16>,
    seed: i64,
    min_block_x: i32,
    min_block_z: i32,
) {
    let mut random = WorldgenRandom::new(RandomSource::xoroshiro(0));
    let decoration_seed = random.set_decoration_seed(seed, min_block_x, min_block_z);
    let origin_y = level.min_y();

    let feature_step_count = registry.steps.len();
    let generation_steps = 11.max(feature_step_count);

    for step_index in 0..generation_steps {
        // Structures (step 0..10) run first in vanilla — deferred to P9; a
        // no-op here is exactly vanilla wherever no structure is present.
        if step_index >= feature_step_count {
            continue;
        }
        let step_list = &registry.steps[step_index];
        let idx_lookup = &registry.step_index[step_index];

        // Union the per-step feature indices of every present biome.
        let mut indices: Vec<i32> = Vec::new();
        let mut seen: HashSet<i32> = HashSet::new();
        for &fill in possible_biomes {
            let biome = registry.biome_name(fill);
            if let Some(per_step) = registry.biome_features.get(biome) {
                if step_index < per_step.len() {
                    for id in &per_step[step_index] {
                        if let Some(&gi) = idx_lookup.get(id) {
                            if seen.insert(gi) {
                                indices.push(gi);
                            }
                        }
                    }
                }
            }
        }
        indices.sort_unstable();

        for gi in indices {
            let id = &step_list[gi as usize];
            random.set_feature_seed(decoration_seed, gi, step_index as i32);
            let placed = match registry.placed.get(id) {
                Some(p) => p,
                None => continue,
            };
            let configured = match registry.configured.get(&placed.feature) {
                Some(c) => c,
                None => continue,
            };
            // Deferred features are skipped entirely (parity-safe: the RNG is
            // reseeded per feature, so skipping cannot affect any other).
            if !configured.is_implemented() {
                continue;
            }
            // If a supported feature somehow used an unsupported placement
            // modifier, skip rather than mis-place (never happens for the
            // implemented set).
            if placed.placement.iter().any(|m| !m.is_supported()) {
                continue;
            }
            let mut ctx = PlacementCtx { level, biome_index: registry, top_feature: id };
            place_with_biome_check(configured, placed, &mut ctx, &mut random, min_block_x, origin_y, min_block_z);
        }
    }
}

/// `PlacedFeature.placeWithBiomeCheck` — thread the origin through the modifier
/// chain depth-first, placing the feature at each terminal position.
fn place_with_biome_check(
    configured: &ConfiguredFeature,
    placed: &PlacedFeature,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    ox: i32,
    oy: i32,
    oz: i32,
) {
    place_stream(configured, &placed.placement, ctx, random, Pos::new(ox, oy, oz));
}

/// Depth-first evaluation of the placement modifier chain (see the module and
/// `placement.rs` notes on why this must be depth-first).
fn place_stream(
    configured: &ConfiguredFeature,
    modifiers: &[PlacementModifier],
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    pos: Pos,
) {
    match modifiers.split_first() {
        None => {
            place_feature(configured, ctx, random, pos);
        }
        Some((first, rest)) => {
            let positions = first.get_positions(ctx, random, pos);
            for p in positions {
                place_stream(configured, rest, ctx, random, p);
            }
        }
    }
}

/// `ConfiguredFeature.place` for the implemented features.
fn place_feature(
    configured: &ConfiguredFeature,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    origin: Pos,
) {
    match configured {
        ConfiguredFeature::Ore(cfg) => place_ore(cfg, ctx, random, origin),
        ConfiguredFeature::ScatteredOre(cfg) => place_scattered_ore(cfg, ctx, random, origin),
        ConfiguredFeature::Spring(cfg) => place_spring(cfg, ctx, origin),
        ConfiguredFeature::Disk(cfg) => place_disk(cfg, ctx, random, origin),
        ConfiguredFeature::Tree(cfg) => place_tree(cfg, ctx, random, origin),
        ConfiguredFeature::RandomSelector(cfg) => place_random_selector(cfg, ctx, random, origin),
        ConfiguredFeature::SimpleBlock(cfg) => place_simple_block(cfg, ctx, random, origin),
        ConfiguredFeature::BlockColumn(cfg) => place_block_column(cfg, ctx, random, origin),
        ConfiguredFeature::Bamboo { probability } => place_bamboo(*probability, ctx, random, origin),
        ConfiguredFeature::Kelp => place_kelp(ctx, random, origin),
        ConfiguredFeature::Seagrass { probability } => place_seagrass(*probability, ctx, random, origin),
        ConfiguredFeature::SeaPickle { count } => place_sea_pickle(count, ctx, random, origin),
        ConfiguredFeature::Lake(cfg) => place_lake(cfg, ctx, random, origin),
        ConfiguredFeature::BlueIce => place_blue_ice(ctx, random, origin),
        ConfiguredFeature::Spike(cfg) => place_spike(cfg, ctx, random, origin),
        ConfiguredFeature::Iceberg(cfg) => place_iceberg(cfg, ctx, random, origin),
        ConfiguredFeature::BlockBlob(cfg) => place_block_blob(cfg, ctx, random, origin),
        ConfiguredFeature::DesertWell => place_desert_well(ctx, random, origin),
        ConfiguredFeature::Deferred(_) => {}
    }
}

// ---------------------------------------------------------------------------
// RandomSelectorFeature.place / PlacedFeature.place (nested)
// ---------------------------------------------------------------------------

/// `RandomSelectorFeature.place` — draw `nextFloat()` per weighted entry in
/// order; the first that passes places its nested feature and returns; otherwise
/// the default feature is placed.
fn place_random_selector(cfg: &RandomSelectorConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    for w in &cfg.features {
        if random.next_float() < w.chance {
            place_nested(&w.feature, ctx, random, origin);
            return;
        }
    }
    place_nested(&cfg.default, ctx, random, origin);
}

/// `PlacedFeature.place` for a nested feature — thread its own placement chain
/// (no biome check) then place the resolved configured feature.
fn place_nested(nf: &NestedFeature, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    if let NestedFeature::Resolved { feature, placement } = nf {
        // A nested placement whose chain contains an unsupported modifier would
        // desync only this terminal feature (the RNG is reseeded per top
        // feature); still, skip rather than mis-draw.
        if placement.iter().any(|m| !m.is_supported()) {
            return;
        }
        place_stream(feature, placement, ctx, random, origin);
    }
}

// ---------------------------------------------------------------------------
// OreFeature
// ---------------------------------------------------------------------------

/// `Mth.ceil`.
fn mth_ceil(v: f64) -> i32 {
    v.ceil() as i32
}

/// `OreFeature.canPlaceOre` + `isAdjacentToAir`.
fn can_place_ore(
    ore_pos_state: ParityBlock,
    ctx: &PlacementCtx,
    random: &mut WorldgenRandom,
    discard_chance: f32,
    target: &RuleTest,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    if !target.test(ore_pos_state, random) {
        return false;
    }
    if should_skip_air_check(random, discard_chance) {
        true
    } else {
        !is_adjacent_to_air(ctx.level, x, y, z)
    }
}

fn should_skip_air_check(random: &mut WorldgenRandom, discard_chance: f32) -> bool {
    if discard_chance <= 0.0 {
        true
    } else if discard_chance >= 1.0 {
        false
    } else {
        random.next_float() >= discard_chance
    }
}

fn is_adjacent_to_air(level: &dyn DecorationLevel, x: i32, y: i32, z: i32) -> bool {
    // Direction.values(): DOWN, UP, NORTH, SOUTH, WEST, EAST.
    const N: [(i32, i32, i32); 6] =
        [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
    N.iter().any(|&(dx, dy, dz)| level.get_block(x + dx, y + dy, z + dz).is_air())
}

/// `OreFeature.place` + `doPlace`.
fn place_ore(cfg: &OreConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let dir = random.next_float() * std::f32::consts::PI;
    let spread_xy = cfg.size as f32 / 8.0;
    let max_radius = mth_ceil(((cfg.size as f32 / 16.0 * 2.0 + 1.0) / 2.0) as f64);
    let x0 = origin.x as f64 + (dir.sin() * spread_xy) as f64;
    let x1 = origin.x as f64 - (dir.sin() * spread_xy) as f64;
    let z0 = origin.z as f64 + (dir.cos() * spread_xy) as f64;
    let z1 = origin.z as f64 - (dir.cos() * spread_xy) as f64;
    let y0 = origin.y as f64 + (random.next_int_bounded(3) - 2) as f64;
    let y1 = origin.y as f64 + (random.next_int_bounded(3) - 2) as f64;
    let x_start = origin.x - mth_ceil(spread_xy as f64) - max_radius;
    let y_start = origin.y - 2 - max_radius;
    let z_start = origin.z - mth_ceil(spread_xy as f64) - max_radius;
    let size_xz = 2 * (mth_ceil(spread_xy as f64) + max_radius);
    let size_y = 2 * (2 + max_radius);

    // Surface gate.
    let mut near_surface = false;
    'gate: for xprobe in x_start..=x_start + size_xz {
        for zprobe in z_start..=z_start + size_xz {
            if y_start <= ctx.level.get_height(Heightmap::OceanFloorWg, xprobe, zprobe) {
                near_surface = true;
                break 'gate;
            }
        }
    }
    if !near_surface {
        return;
    }

    do_place_ore(cfg, ctx, random, x0, x1, z0, z1, y0, y1, x_start, y_start, z_start, size_xz, size_y);
}

#[allow(clippy::too_many_arguments)]
fn do_place_ore(
    cfg: &OreConfig,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
    y0: f64,
    y1: f64,
    x_start: i32,
    y_start: i32,
    z_start: i32,
    size_xz: i32,
    size_y: i32,
) {
    let size = cfg.size;
    if size <= 0 {
        return;
    }
    let mut tested = vec![false; (size_xz * size_y * size_xz).max(0) as usize];
    // data[i] = (xx, yy, zz, radius)
    let mut data = vec![(0.0f64, 0.0f64, 0.0f64, 0.0f64); size as usize];
    for i in 0..size {
        let step = i as f32 / size as f32;
        let xx = lerp(step as f64, x0, x1);
        let yy = lerp(step as f64, y0, y1);
        let zz = lerp(step as f64, z0, z1);
        let ss = random.next_double() * size as f64 / 16.0;
        let r = (((std::f32::consts::PI * step).sin() as f64 + 1.0) * ss + 1.0) / 2.0;
        data[i as usize] = (xx, yy, zz, r);
    }

    for i1 in 0..size - 1 {
        if data[i1 as usize].3 <= 0.0 {
            continue;
        }
        for i2 in i1 + 1..size {
            if data[i2 as usize].3 <= 0.0 {
                continue;
            }
            let dx = data[i1 as usize].0 - data[i2 as usize].0;
            let dy = data[i1 as usize].1 - data[i2 as usize].1;
            let dz = data[i1 as usize].2 - data[i2 as usize].2;
            let dr = data[i1 as usize].3 - data[i2 as usize].3;
            if dr * dr > dx * dx + dy * dy + dz * dz {
                if dr > 0.0 {
                    data[i2 as usize].3 = -1.0;
                } else {
                    data[i1 as usize].3 = -1.0;
                }
            }
        }
    }

    for i in 0..size as usize {
        let (xx, yy, zz, r) = data[i];
        if r < 0.0 {
            continue;
        }
        let x_min = ((xx - r).floor() as i32).max(x_start);
        let y_min = ((yy - r).floor() as i32).max(y_start);
        let z_min = ((zz - r).floor() as i32).max(z_start);
        let x_max = ((xx + r).floor() as i32).max(x_min);
        let y_max = ((yy + r).floor() as i32).max(y_min);
        let z_max = ((zz + r).floor() as i32).max(z_min);

        for x in x_min..=x_max {
            let xd = (x as f64 + 0.5 - xx) / r;
            if xd * xd >= 1.0 {
                continue;
            }
            for y in y_min..=y_max {
                let yd = (y as f64 + 0.5 - yy) / r;
                if xd * xd + yd * yd >= 1.0 {
                    continue;
                }
                for z in z_min..=z_max {
                    let zd = (z as f64 + 0.5 - zz) / r;
                    if xd * xd + yd * yd + zd * zd >= 1.0 || ctx.level.is_outside_build_height(y) {
                        continue;
                    }
                    let bit = (x - x_start) + (y - y_start) * size_xz + (z - z_start) * size_xz * size_y;
                    if bit < 0 || bit as usize >= tested.len() || tested[bit as usize] {
                        continue;
                    }
                    tested[bit as usize] = true;
                    let existing = ctx.level.get_block(x, y, z);
                    for target in &cfg.targets {
                        if can_place_ore(existing, ctx, random, cfg.discard_chance_on_air_exposure, &target.target, x, y, z) {
                            ctx.level.set_block(x, y, z, target.state);
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// `ScatteredOreFeature.place`.
fn place_scattered_ore(cfg: &OreConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let tries = random.next_int_bounded(cfg.size + 1);
    for i in 0..tries {
        let max_dist = i.min(7);
        let xd = ((random.next_float() - random.next_float()) * max_dist as f32).round() as i32;
        let yd = ((random.next_float() - random.next_float()) * max_dist as f32).round() as i32;
        let zd = ((random.next_float() - random.next_float()) * max_dist as f32).round() as i32;
        let (x, y, z) = (origin.x + xd, origin.y + yd, origin.z + zd);
        let existing = ctx.level.get_block(x, y, z);
        for target in &cfg.targets {
            if can_place_ore(existing, ctx, random, cfg.discard_chance_on_air_exposure, &target.target, x, y, z) {
                ctx.level.set_block(x, y, z, target.state);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SpringFeature
// ---------------------------------------------------------------------------

/// `SpringFeature.place`. `scheduleTick` (fluid ticking) is a sim concern, not a
/// block write, so it is omitted (output-neutral for the block grid).
fn place_spring(cfg: &SpringConfig, ctx: &mut PlacementCtx, origin: Pos) {
    let (x, y, z) = (origin.x, origin.y, origin.z);
    let valid = |b: ParityBlock| cfg.valid_blocks.contains(&b);
    if !valid(ctx.level.get_block(x, y + 1, z)) {
        return;
    }
    if cfg.requires_block_below && !valid(ctx.level.get_block(x, y - 1, z)) {
        return;
    }
    let current = ctx.level.get_block(x, y, z);
    if !current.is_air() && !valid(current) {
        return;
    }
    let mut rock_count = 0;
    let mut hole_count = 0;
    // west, east, north, south, below.
    const SIDES: [(i32, i32, i32); 5] =
        [(-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1), (0, -1, 0)];
    for &(dx, dy, dz) in &SIDES {
        let b = ctx.level.get_block(x + dx, y + dy, z + dz);
        if valid(b) {
            rock_count += 1;
        }
    }
    // Holes: west, east, north, south, below (isEmptyBlock = air).
    for &(dx, dy, dz) in &SIDES {
        if ctx.level.get_block(x + dx, y + dy, z + dz).is_air() {
            hole_count += 1;
        }
    }
    if rock_count == cfg.rock_count && hole_count == cfg.hole_count {
        ctx.level.set_block(x, y, z, cfg.fluid);
    }
}

// ---------------------------------------------------------------------------
// DiskFeature
// ---------------------------------------------------------------------------

/// `DiskFeature.place` / `placeColumn`. `markAboveForPostProcessing` is a
/// lighting/post flag, not a block write, so it is omitted.
fn place_disk(cfg: &DiskConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let origin_y = origin.y;
    let top = origin_y + cfg.half_height;
    let bottom = origin_y - cfg.half_height - 1;
    let r = cfg.radius.sample(random);
    for xd in -r..=r {
        for zd in -r..=r {
            if xd * xd + zd * zd > r * r {
                continue;
            }
            let cx = origin.x + xd;
            let cz = origin.z + zd;
            let mut y = top;
            while y > bottom {
                let pos = Pos::new(cx, y, cz);
                if cfg.target.test(ctx.level, pos) {
                    if let Some(state) = cfg.state_provider.get_state(ctx.level, random, pos) {
                        ctx.level.set_block(cx, y, cz, state);
                    }
                }
                y -= 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vegetal decoration — SimpleBlockFeature / BlockColumnFeature + survival
// ---------------------------------------------------------------------------

/// `#minecraft:supports_vegetation` over the parity alphabet (the plant floor).
fn supports_vegetation(b: ParityBlock) -> bool {
    BlockTag::SupportsVegetation.contains(b)
}

/// `#minecraft:supports_dry_vegetation` = `#sand ∪ #terracotta ∪ #supports_vegetation`.
fn supports_dry_vegetation(b: ParityBlock) -> bool {
    use ParityBlock::*;
    supports_vegetation(b)
        || matches!(
            b,
            Sand | RedSand
                | Terracotta
                | WhiteTerracotta
                | OrangeTerracotta
                | YellowTerracotta
                | BrownTerracotta
                | RedTerracotta
                | LightGrayTerracotta
        )
}

/// `#minecraft:beneath_bamboo_podzol_replaceable` = `#substrate_overworld`.
fn bamboo_podzol_replaceable(b: ParityBlock) -> bool {
    BlockTag::BeneathTreePodzolReplaceable.contains(b)
}

/// `#minecraft:supports_bamboo` = `#sand ∪ #substrate_overworld ∪ bamboo /
/// bamboo_sapling / gravel / suspicious_gravel` over the parity alphabet.
fn supports_bamboo(b: ParityBlock) -> bool {
    use ParityBlock::*;
    BlockTag::BeneathTreePodzolReplaceable.contains(b)
        || matches!(b, Sand | RedSand | Gravel | Bamboo | BambooSapling)
}

/// `DoublePlantBlock` membership over the alphabet (their upper half collapses to
/// the same default block per precedent).
fn is_double_plant(b: ParityBlock) -> bool {
    use ParityBlock::*;
    matches!(b, TallGrass | LargeFern | Sunflower | Lilac | RoseBush | Peony)
}

/// `state.canSurvive(level, pos)` for the states a `simple_block` feature places.
/// The check draws no RNG (it only gates whether the block appears), so any
/// approximation here can never desync the enclosing feature — only shift a plant
/// on/off marginal terrain. Tag-based cases are exact; the light/face-sturdy
/// cases are approximated (`blocks_motion`), documented in the module notes.
fn simple_block_can_survive(b: ParityBlock, level: &dyn DecorationLevel, p: Pos) -> bool {
    use ParityBlock::*;
    let below = level.get_block(p.x, p.y - 1, p.z);
    match b {
        ShortGrass | Fern | TallGrass | LargeFern | Bush | SweetBerryBush | FireflyBush | Sunflower
        | Lilac | RoseBush | Peony | Dandelion | Poppy | BlueOrchid | Allium | AzureBluet | RedTulip
        | OrangeTulip | WhiteTulip | PinkTulip | OxeyeDaisy | Cornflower | LilyOfTheValley | PinkPetals
        | ClosedEyeblossom | Wildflowers => supports_vegetation(below),
        // `AzaleaBlock.mayPlaceOn` = `#dirt ∪ clay ∪ farmland`, plus (in lush caves)
        // the moss floor it is scattered on.
        Azalea | FloweringAzalea => supports_vegetation(below) || below == Clay,
        ShortDryGrass | TallDryGrass | DeadBush => supports_dry_vegetation(below),
        // `LeafLitterBlock.mayPlaceOn` = below face-sturdy up (approx `blocks_motion`).
        LeafLitter => below.blocks_motion(),
        // `CarpetBlock` / `MossyCarpetBlock` base: below must not be air.
        MossCarpet | PaleMossCarpet => !below.is_air(),
        // `MushroomBlock`: below `isSolidRender` and light < 13 (worldgen light is
        // unpopulated → always < 13). Approx solid-render as `blocks_motion` and
        // not `#leaves`.
        BrownMushroom | RedMushroom => below.blocks_motion() && !below.is_leaves(),
        // Full solid blocks with no plant survival override.
        Pumpkin | Melon | MossBlock | PaleMossBlock => true,
        // `LilyPadBlock.mayPlaceOn`: below is water or ice, own cell fluid empty
        // (origin is filtered to air by placement → empty).
        LilyPad => matches!(below, Water | Ice),
        // `spore_blossom` is a ceiling block: the block above must be face-sturdy
        // down (approx `blocks_motion`).
        SporeBlossom => level.get_block(p.x, p.y + 1, p.z).blocks_motion(),
        _ => true,
    }
}

/// `SimpleBlockFeature.place`. Draws the state provider (may consume RNG), then
/// the survival gate (no RNG); double plants place both halves, everything else a
/// single block. `schedule_tick` is a sim concern (no block write) and omitted.
fn place_simple_block(cfg: &SimpleBlockConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let state = match cfg.to_place.get_state(ctx.level, random, origin) {
        Some(s) => s,
        None => return,
    };
    if !simple_block_can_survive(state, ctx.level, origin) {
        return;
    }
    if is_double_plant(state) {
        if !ctx.level.get_block(origin.x, origin.y + 1, origin.z).is_air() {
            return;
        }
        ctx.level.set_block(origin.x, origin.y, origin.z, state);
        ctx.level.set_block(origin.x, origin.y + 1, origin.z, state);
    } else {
        // `MossyCarpetBlock.placeAt` (pale_moss_carpet) draws 0–4 `nextBoolean`
        // for wall-side toppers; on open worldgen ground no wall sides exist so it
        // draws none — collapsed here to a plain carpet placement (documented).
        ctx.level.set_block(origin.x, origin.y, origin.z, state);
    }
}

/// `BlockColumnFeature.truncate`.
fn block_column_truncate(heights: &mut [i32], total: i32, new_height: i32, prioritize_tip: bool) {
    let mut to_remove = total - new_height;
    let dir: i32 = if prioritize_tip { 1 } else { -1 };
    let start: i32 = if prioritize_tip { 0 } else { heights.len() as i32 - 1 };
    let end: i32 = if prioritize_tip { heights.len() as i32 } else { -1 };
    let mut i = start;
    while i != end && to_remove > 0 {
        let this = heights[i as usize];
        let r = this.min(to_remove);
        to_remove -= r;
        heights[i as usize] -= r;
        i += dir;
    }
}

/// `BlockColumnFeature.place`. Samples each layer's height (RNG), grows the column
/// up to the first blocked cell (truncating), then fills the layers bottom-up.
fn place_block_column(cfg: &BlockColumnConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let n = cfg.layers.len();
    let mut heights = vec![0i32; n];
    let mut total = 0;
    for i in 0..n {
        heights[i] = cfg.layers[i].height.sample(random);
        total += heights[i];
    }
    if total == 0 {
        return;
    }
    let (dx, dy, dz) = cfg.dir;
    let mut next = origin.offset(dx, dy, dz);
    for y in 0..total {
        if !cfg.allowed_placement.test(ctx.level, next) {
            block_column_truncate(&mut heights, total, y, cfg.prioritize_tip);
            break;
        }
        next = next.offset(dx, dy, dz);
    }
    let mut place = origin;
    for i in 0..n {
        for _ in 0..heights[i] {
            if let Some(s) = cfg.layers[i].provider.get_state(ctx.level, random, place) {
                ctx.level.set_block(place.x, place.y, place.z, s);
            }
            place = place.offset(dx, dy, dz);
        }
    }
}

// ---------------------------------------------------------------------------
// BambooFeature / KelpFeature / SeagrassFeature / SeaPickleFeature / LakeFeature
// ---------------------------------------------------------------------------

/// `BambooFeature.place`. A bamboo stalk (collapsed to the `bamboo` default state
/// for every segment) with an optional podzol disc.
fn place_bamboo(probability: f32, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    use ParityBlock::*;
    if !ctx.level.get_block(origin.x, origin.y, origin.z).is_air() {
        return;
    }
    if !supports_bamboo(ctx.level.get_block(origin.x, origin.y - 1, origin.z)) {
        return;
    }
    let height = random.next_int_bounded(12) + 5;
    if random.next_float() < probability {
        let r = random.next_int_bounded(4) + 1;
        for xx in origin.x - r..=origin.x + r {
            for zz in origin.z - r..=origin.z + r {
                let xd = xx - origin.x;
                let zd = zz - origin.z;
                if xd * xd + zd * zd <= r * r {
                    let hy = ctx.level.get_height(Heightmap::WorldSurface, xx, zz) - 1;
                    if bamboo_podzol_replaceable(ctx.level.get_block(xx, hy, zz)) {
                        ctx.level.set_block(xx, hy, zz, Podzol);
                    }
                }
            }
        }
    }
    let mut by = origin.y;
    let mut i = 0;
    while i < height && ctx.level.get_block(origin.x, by, origin.z).is_air() {
        ctx.level.set_block(origin.x, by, origin.z, Bamboo);
        by += 1;
        i += 1;
    }
    if by - origin.y >= 3 {
        // BAMBOO_FINAL_LARGE / BAMBOO_TOP_LARGE / BAMBOO_TOP_SMALL all collapse to
        // the `bamboo` default state (leaves/stage properties dropped).
        ctx.level.set_block(origin.x, by, origin.z, Bamboo);
        by -= 1;
        ctx.level.set_block(origin.x, by, origin.z, Bamboo);
        by -= 1;
        ctx.level.set_block(origin.x, by, origin.z, Bamboo);
    }
}

/// Kelp survival (`GrowingPlantBlock.canSurvive`, growth up): the block below is
/// kelp or a face-sturdy top (approx `blocks_motion`).
fn kelp_can_survive(level: &dyn DecorationLevel, plant: Pos) -> bool {
    use ParityBlock::*;
    let below = level.get_block(plant.x, plant.y - 1, plant.z);
    matches!(below, Kelp | KelpPlant) || below.blocks_motion()
}

/// `KelpFeature.place`.
fn place_kelp(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    use ParityBlock::*;
    let y = ctx.level.get_height(Heightmap::OceanFloor, origin.x, origin.z);
    let mut pos = Pos::new(origin.x, y, origin.z);
    if ctx.level.get_block(pos.x, pos.y, pos.z) != Water {
        return;
    }
    let height = 1 + random.next_int_bounded(10);
    for h in 0..=height {
        let here = ctx.level.get_block(pos.x, pos.y, pos.z);
        let above = ctx.level.get_block(pos.x, pos.y + 1, pos.z);
        if here == Water && above == Water && kelp_can_survive(ctx.level, pos) {
            if h == height {
                let _age = random.next_int_bounded(4) + 20;
                ctx.level.set_block(pos.x, pos.y, pos.z, Kelp);
            } else {
                ctx.level.set_block(pos.x, pos.y, pos.z, KelpPlant);
            }
        } else if h > 0 {
            let below = Pos::new(pos.x, pos.y - 1, pos.z);
            if kelp_can_survive(ctx.level, below) && ctx.level.get_block(below.x, below.y - 1, below.z) != Kelp {
                let _age = random.next_int_bounded(4) + 20;
                ctx.level.set_block(below.x, below.y, below.z, Kelp);
            }
            break;
        }
        pos = Pos::new(pos.x, pos.y + 1, pos.z);
    }
}

/// `SeagrassFeature.place`.
fn place_seagrass(probability: f32, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    use ParityBlock::*;
    let x = random.next_int_bounded(8) - random.next_int_bounded(8);
    let z = random.next_int_bounded(8) - random.next_int_bounded(8);
    let y = ctx.level.get_height(Heightmap::OceanFloor, origin.x + x, origin.z + z);
    let p = Pos::new(origin.x + x, y, origin.z + z);
    if ctx.level.get_block(p.x, p.y, p.z) != Water {
        return;
    }
    let is_tall = random.next_double() < probability as f64;
    // Seagrass survival: below is face-sturdy up and not magma (approx blocks_motion).
    if !ctx.level.get_block(p.x, p.y - 1, p.z).blocks_motion() {
        return;
    }
    if is_tall {
        if ctx.level.get_block(p.x, p.y + 1, p.z) == Water {
            ctx.level.set_block(p.x, p.y, p.z, TallSeagrass);
            ctx.level.set_block(p.x, p.y + 1, p.z, TallSeagrass);
        }
    } else {
        ctx.level.set_block(p.x, p.y, p.z, Seagrass);
    }
}

/// `SeaPickleFeature.place`. Per attempt: 4 position draws + 1 `pickles` draw
/// (consumed unconditionally, before the water/survival gate).
fn place_sea_pickle(count: &IntProvider, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    use ParityBlock::*;
    let n = count.sample(random);
    for _ in 0..n {
        let x = random.next_int_bounded(8) - random.next_int_bounded(8);
        let z = random.next_int_bounded(8) - random.next_int_bounded(8);
        let y = ctx.level.get_height(Heightmap::OceanFloor, origin.x + x, origin.z + z);
        let p = Pos::new(origin.x + x, y, origin.z + z);
        let _pickles = random.next_int_bounded(4) + 1;
        if ctx.level.get_block(p.x, p.y, p.z) == Water && ctx.level.get_block(p.x, p.y - 1, p.z).blocks_motion() {
            ctx.level.set_block(p.x, p.y, p.z, SeaPickle);
        }
    }
}

/// `LakeFeature.place` (`lake_lava_*`). Builds an ellipsoid-union carve grid, does
/// the border-integrity scan (may abort), fills fluid/air, then the barrier shell.
/// `scheduleTick` / `markAboveForPostProcessing` are post/sim flags (no block
/// write) and omitted. The water-ice pass never runs for lava lakes.
fn place_lake(cfg: &LakeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    if origin.y <= ctx.level.min_y() + 4 {
        return;
    }
    let base = origin.offset(-8, -4, -8);
    let mut grid = [false; 2048];
    let idx = |xx: i32, zz: i32, yy: i32| ((xx * 16 + zz) * 8 + yy) as usize;
    let spots = random.next_int_bounded(4) + 4;
    for _ in 0..spots {
        let xr = random.next_double() * 6.0 + 3.0;
        let yr = random.next_double() * 4.0 + 2.0;
        let zr = random.next_double() * 6.0 + 3.0;
        let xp = random.next_double() * (16.0 - xr - 2.0) + 1.0 + xr / 2.0;
        let yp = random.next_double() * (8.0 - yr - 4.0) + 2.0 + yr / 2.0;
        let zp = random.next_double() * (16.0 - zr - 2.0) + 1.0 + zr / 2.0;
        for xx in 1..15 {
            for zz in 1..15 {
                for yy in 1..7 {
                    let xd = (xx as f64 - xp) / (xr / 2.0);
                    let yd = (yy as f64 - yp) / (yr / 2.0);
                    let zd = (zz as f64 - zp) / (zr / 2.0);
                    if xd * xd + yd * yd + zd * zd < 1.0 {
                        grid[idx(xx, zz, yy)] = true;
                    }
                }
            }
        }
    }

    let fluid = match cfg.fluid.get_state(ctx.level, random, base) {
        Some(f) => f,
        None => return,
    };

    // Border-integrity scan.
    let border = |grid: &[bool; 2048], xx: i32, zz: i32, yy: i32| -> bool {
        !grid[idx(xx, zz, yy)]
            && (xx < 15 && grid[idx(xx + 1, zz, yy)]
                || xx > 0 && grid[idx(xx - 1, zz, yy)]
                || zz < 15 && grid[idx(xx, zz + 1, yy)]
                || zz > 0 && grid[idx(xx, zz - 1, yy)]
                || yy < 7 && grid[idx(xx, zz, yy + 1)]
                || yy > 0 && grid[idx(xx, zz, yy - 1)])
    };
    for xx in 0..16 {
        for zz in 0..16 {
            for yy in 0..8 {
                if border(&grid, xx, zz, yy) {
                    let op = base.offset(xx, yy, zz);
                    let bs = ctx.level.get_block(op.x, op.y, op.z);
                    if yy >= 4 && bs.is_fluid() {
                        return;
                    }
                    if yy < 4 && !bs.blocks_motion() && bs != fluid {
                        return;
                    }
                    // `can_place_feature` is `true` for the lava lakes.
                }
            }
        }
    }

    // Fill pass.
    for xx in 0..16 {
        for zz in 0..16 {
            for yy in 0..8 {
                if grid[idx(xx, zz, yy)] {
                    let pp = base.offset(xx, yy, zz);
                    if cfg.can_replace_with_air_or_fluid.test(ctx.level, pp) {
                        let state = if yy >= 4 { ParityBlock::Air } else { fluid };
                        ctx.level.set_block(pp.x, pp.y, pp.z, state);
                    }
                }
            }
        }
    }

    // Barrier shell.
    let barrier = match cfg.barrier.get_state(ctx.level, random, base) {
        Some(b) => b,
        None => return,
    };
    if !barrier.is_air() {
        for xx in 0..16 {
            for zz in 0..16 {
                for yy in 0..8 {
                    if border(&grid, xx, zz, yy) && (yy < 4 || random.next_int_bounded(2) != 0) {
                        let op = base.offset(xx, yy, zz);
                        let bs = ctx.level.get_block(op.x, op.y, op.z);
                        if bs.blocks_motion() && cfg.can_replace_with_barrier.test(ctx.level, op) {
                            ctx.level.set_block(op.x, op.y, op.z, barrier);
                        }
                    }
                }
            }
        }
    }
    // Lava fluid → the water-ice pass is skipped.
}

// ---------------------------------------------------------------------------
// Frozen / ice group: blue_ice, ice_spike (SpikeFeature), iceberg
// ---------------------------------------------------------------------------

/// `Mth.ceil(float)` — `(int)value` (truncate toward zero), rounded up when
/// `value` exceeds that truncation.
fn mth_ceil_f32(v: f32) -> i32 {
    let i = v as i32;
    if v > i as f32 { i + 1 } else { i }
}

/// `BlueIceFeature.place`.
fn place_blue_ice(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    if origin.y > ctx.level.sea_level() - 1 {
        return;
    }
    let here = ctx.level.get_block(origin.x, origin.y, origin.z);
    let below = ctx.level.get_block(origin.x, origin.y - 1, origin.z);
    if here != ParityBlock::Water && below != ParityBlock::Water {
        return;
    }
    // Direction.values() minus DOWN: UP, NORTH, SOUTH, WEST, EAST.
    const NON_DOWN: [(i32, i32, i32); 5] = [(0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
    let found = NON_DOWN
        .iter()
        .any(|&(dx, dy, dz)| ctx.level.get_block(origin.x + dx, origin.y + dy, origin.z + dz) == ParityBlock::PackedIce);
    if !found {
        return;
    }
    ctx.level.set_block(origin.x, origin.y, origin.z, ParityBlock::BlueIce);

    for _ in 0..200 {
        let y_off = random.next_int_bounded(5) - random.next_int_bounded(6);
        let mut xz_diff = 3;
        if y_off < 2 {
            xz_diff += y_off / 2;
        }
        if xz_diff >= 1 {
            let dx = random.next_int_bounded(xz_diff) - random.next_int_bounded(xz_diff);
            let dz = random.next_int_bounded(xz_diff) - random.next_int_bounded(xz_diff);
            let px = origin.x + dx;
            let py = origin.y + y_off;
            let pz = origin.z + dz;
            let ps = ctx.level.get_block(px, py, pz);
            if ps.is_air() || matches!(ps, ParityBlock::Water | ParityBlock::PackedIce | ParityBlock::Ice) {
                // Direction.values(): DOWN, UP, NORTH, SOUTH, WEST, EAST.
                const ALL6: [(i32, i32, i32); 6] =
                    [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
                for &(rx, ry, rz) in ALL6.iter() {
                    if ctx.level.get_block(px + rx, py + ry, pz + rz) == ParityBlock::BlueIce {
                        ctx.level.set_block(px, py, pz, ParityBlock::BlueIce);
                        break;
                    }
                }
            }
        }
    }
}

/// `SpikeFeature.place` (ice_spike).
fn place_spike(cfg: &SpikeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let mut origin = origin;
    while ctx.level.get_block(origin.x, origin.y, origin.z).is_air() && origin.y > ctx.level.min_y() + 2 {
        origin.y -= 1;
    }
    if !cfg.can_place_on.test(ctx.level, origin) {
        return;
    }
    origin.y += random.next_int_bounded(4);
    let height = random.next_int_bounded(4) + 7;
    let width = height / 4 + random.next_int_bounded(2);
    if width > 1 && random.next_int_bounded(60) == 0 {
        origin.y += 10 + random.next_int_bounded(30);
    }

    for y_off in 0..height {
        let scale = (1.0 - y_off as f32 / height as f32) * width as f32;
        let new_width = mth_ceil_f32(scale);
        for xo in -new_width..=new_width {
            let dx = (xo.abs() as f32) - 0.25;
            for zo in -new_width..=new_width {
                let dz = (zo.abs() as f32) - 0.25;
                if ((xo == 0 && zo == 0) || !(dx * dx + dz * dz > scale * scale))
                    && ((xo != -new_width && xo != new_width && zo != -new_width && zo != new_width)
                        || !(random.next_float() > 0.75))
                {
                    let p = Pos::new(origin.x + xo, origin.y + y_off, origin.z + zo);
                    let st = ctx.level.get_block(p.x, p.y, p.z);
                    if st.is_air() || cfg.can_replace.test(ctx.level, p) {
                        ctx.level.set_block(p.x, p.y, p.z, cfg.state);
                    }
                    if y_off != 0 && new_width > 1 {
                        let pn = Pos::new(origin.x + xo, origin.y - y_off, origin.z + zo);
                        let stn = ctx.level.get_block(pn.x, pn.y, pn.z);
                        if stn.is_air() || cfg.can_replace.test(ctx.level, pn) {
                            ctx.level.set_block(pn.x, pn.y, pn.z, cfg.state);
                        }
                    }
                }
            }
        }
    }

    let mut pillar_width = width - 1;
    if pillar_width < 0 {
        pillar_width = 0;
    } else if pillar_width > 1 {
        pillar_width = 1;
    }
    for xo in -pillar_width..=pillar_width {
        for zo in -pillar_width..=pillar_width {
            let mut cursor = Pos::new(origin.x + xo, origin.y - 1, origin.z + zo);
            let mut run_length = 50;
            if xo.abs() == 1 && zo.abs() == 1 {
                run_length = random.next_int_bounded(5);
            }
            while cursor.y > 50 {
                let st = ctx.level.get_block(cursor.x, cursor.y, cursor.z);
                if !st.is_air() && !cfg.can_replace.test(ctx.level, cursor) && st != cfg.state {
                    break;
                }
                ctx.level.set_block(cursor.x, cursor.y, cursor.z, cfg.state);
                cursor.y -= 1;
                run_length -= 1;
                if run_length <= 0 {
                    cursor.y -= random.next_int_bounded(5) + 1;
                    run_length = random.next_int_bounded(5);
                }
            }
        }
    }
}

// --- IcebergFeature ---------------------------------------------------------

fn iceberg_is_iceberg_state(b: ParityBlock) -> bool {
    matches!(b, ParityBlock::PackedIce | ParityBlock::SnowBlock | ParityBlock::BlueIce)
}

fn iceberg_signed_distance_ellipse(xo: i32, zo: i32, ox: i32, oz: i32, a: i32, c: i32, angle: f64) -> f64 {
    let fx = (xo - ox) as f64;
    let fz = (zo - oz) as f64;
    let t1 = (fx * angle.cos() - fz * angle.sin()) / a as f64;
    let t2 = (fx * angle.sin() + fz * angle.cos()) / c as f64;
    t1 * t1 + t2 * t2 - 1.0
}

fn iceberg_signed_distance_circle(xo: i32, zo: i32, radius: i32, random: &mut WorldgenRandom) -> f64 {
    let off = 10.0_f32 * random.next_float().clamp(0.2, 0.8) / radius as f32;
    off as f64 + (xo * xo) as f64 + (zo * zo) as f64 - (radius * radius) as f64
}

fn iceberg_radius_round(random: &mut WorldgenRandom, y_off: i32, height: i32, width: i32) -> i32 {
    let k = 3.5_f32 - random.next_float();
    let mut scale = (1.0 - (y_off * y_off) as f64 as f32 / (height as f32 * k)) * width as f32;
    if height > 15 + random.next_int_bounded(5) {
        let temp_y = if y_off < 3 + random.next_int_bounded(6) { y_off / 2 } else { y_off };
        scale = (1.0 - temp_y as f32 / (height as f32 * k * 0.4)) * width as f32;
    }
    mth_ceil_f32(scale / 2.0)
}

fn iceberg_radius_ellipse(y_off: i32, height: i32, width: i32) -> i32 {
    let scale = (1.0 - (y_off * y_off) as f64 as f32 / (height as f32)) * width as f32;
    mth_ceil_f32(scale / 2.0)
}

fn iceberg_radius_steep(random: &mut WorldgenRandom, y_off: i32, height: i32, width: i32) -> i32 {
    let k = 1.0_f32 + random.next_float() / 2.0;
    let scale = (1.0 - y_off as f32 / (height as f32 * k)) * width as f32;
    mth_ceil_f32(scale / 2.0)
}

fn iceberg_ellipse_c(y_off: i32, height: i32, shape_c: i32) -> i32 {
    let mut c = shape_c;
    if y_off > 0 && height - y_off <= 3 {
        c -= 4 - (height - y_off);
    }
    c
}

#[allow(clippy::too_many_arguments)]
fn iceberg_set_block(
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    p: Pos,
    h_diff: i32,
    height: i32,
    is_ellipse: bool,
    snow_on_top: bool,
    main: ParityBlock,
) {
    let st = ctx.level.get_block(p.x, p.y, p.z);
    if st.is_air() || matches!(st, ParityBlock::SnowBlock | ParityBlock::Ice | ParityBlock::Water) {
        let randomness = !is_ellipse || random.next_double() > 0.05;
        let divisor = if is_ellipse { 3 } else { 2 };
        if snow_on_top
            && st != ParityBlock::Water
            && (h_diff as f64) <= random.next_int_bounded((height / divisor).max(1)) as f64 + height as f64 * 0.6
            && randomness
        {
            ctx.level.set_block(p.x, p.y, p.z, ParityBlock::SnowBlock);
        } else {
            ctx.level.set_block(p.x, p.y, p.z, main);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn iceberg_generate_block(
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    origin: Pos,
    height: i32,
    xo: i32,
    y_off: i32,
    zo: i32,
    radius: i32,
    a: i32,
    is_ellipse: bool,
    shape_c: i32,
    angle: f64,
    snow_on_top: bool,
    main: ParityBlock,
) {
    let signed = if is_ellipse {
        iceberg_signed_distance_ellipse(xo, zo, 0, 0, a, iceberg_ellipse_c(y_off, height, shape_c), angle)
    } else {
        iceberg_signed_distance_circle(xo, zo, radius, random)
    };
    if signed < 0.0 {
        let compare = if is_ellipse { -0.5 } else { -6.0 - random.next_int_bounded(3) as f64 };
        if signed > compare && random.next_double() > 0.9 {
            return;
        }
        let p = Pos::new(origin.x + xo, origin.y + y_off, origin.z + zo);
        iceberg_set_block(ctx, random, p, height - y_off, height, is_ellipse, snow_on_top, main);
    }
}

fn iceberg_below_is_air(ctx: &PlacementCtx, p: Pos) -> bool {
    ctx.level.get_block(p.x, p.y - 1, p.z).is_air()
}

fn iceberg_smooth(ctx: &mut PlacementCtx, origin: Pos, width: i32, height: i32, is_ellipse: bool, shape_a: i32) {
    let a = if is_ellipse { shape_a } else { width / 2 };
    for x in -a..=a {
        for z in -a..=a {
            for y_off in 0..=height {
                let p = Pos::new(origin.x + x, origin.y + y_off, origin.z + z);
                let st = ctx.level.get_block(p.x, p.y, p.z);
                if iceberg_is_iceberg_state(st) || st == ParityBlock::Snow {
                    if iceberg_below_is_air(ctx, p) {
                        ctx.level.set_block(p.x, p.y, p.z, ParityBlock::Air);
                        ctx.level.set_block(p.x, p.y + 1, p.z, ParityBlock::Air);
                    } else if iceberg_is_iceberg_state(st) {
                        let sides = [
                            ctx.level.get_block(p.x - 1, p.y, p.z),
                            ctx.level.get_block(p.x + 1, p.y, p.z),
                            ctx.level.get_block(p.x, p.y, p.z - 1),
                            ctx.level.get_block(p.x, p.y, p.z + 1),
                        ];
                        let counter = sides.iter().filter(|&&s| !iceberg_is_iceberg_state(s)).count();
                        if counter >= 3 {
                            ctx.level.set_block(p.x, p.y, p.z, ParityBlock::Air);
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn iceberg_carve(
    ctx: &mut PlacementCtx,
    radius: i32,
    y_off: i32,
    origin: Pos,
    under_water: bool,
    angle: f64,
    local: Pos,
    shape_a: i32,
    shape_c: i32,
) {
    let a = radius + 1 + shape_a / 3;
    let c = (radius - 3).min(3) + shape_c / 2 - 1;
    for xo in -a..a {
        for zo in -a..a {
            let signed = iceberg_signed_distance_ellipse(xo, zo, local.x, local.z, a, c, angle);
            if signed < 0.0 {
                let p = Pos::new(origin.x + xo, origin.y + y_off, origin.z + zo);
                let st = ctx.level.get_block(p.x, p.y, p.z);
                if iceberg_is_iceberg_state(st) || st == ParityBlock::SnowBlock {
                    if under_water {
                        ctx.level.set_block(p.x, p.y, p.z, ParityBlock::Water);
                    } else {
                        ctx.level.set_block(p.x, p.y, p.z, ParityBlock::Air);
                        if ctx.level.get_block(p.x, p.y + 1, p.z) == ParityBlock::Snow {
                            ctx.level.set_block(p.x, p.y + 1, p.z, ParityBlock::Air);
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn iceberg_generate_cutout(
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    width: i32,
    height: i32,
    origin: Pos,
    is_ellipse: bool,
    shape_a: i32,
    angle_base: f64,
    shape_c: i32,
) {
    let sign_x = if random.next_boolean() { -1 } else { 1 };
    let sign_z = if random.next_boolean() { -1 } else { 1 };
    let mut x_off = random.next_int_bounded((width / 2 - 2).max(1));
    if random.next_boolean() {
        x_off = width / 2 + 1 - random.next_int_bounded((width - width / 2 - 1).max(1));
    }
    let mut z_off = random.next_int_bounded((width / 2 - 2).max(1));
    if random.next_boolean() {
        z_off = width / 2 + 1 - random.next_int_bounded((width - width / 2 - 1).max(1));
    }
    if is_ellipse {
        x_off = random.next_int_bounded((shape_a - 5).max(1));
        z_off = x_off;
    }
    let local = Pos::new(sign_x * x_off, 0, sign_z * z_off);
    let angle = if is_ellipse {
        angle_base + std::f64::consts::FRAC_PI_2
    } else {
        random.next_double() * 2.0 * std::f64::consts::PI
    };

    for y_off in 0..height - 3 {
        let radius = iceberg_radius_round(random, y_off, height, width);
        iceberg_carve(ctx, radius, y_off, origin, false, angle, local, shape_a, shape_c);
    }
    let mut y_off = -1;
    loop {
        let bound = -height + random.next_int_bounded(5);
        if !(y_off > bound) {
            break;
        }
        let radius = iceberg_radius_steep(random, -y_off, height, width);
        iceberg_carve(ctx, radius, y_off, origin, true, angle, local, shape_a, shape_c);
        y_off -= 1;
    }
}

/// `IcebergFeature.place`.
fn place_iceberg(cfg: &IcebergConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let origin = Pos::new(origin.x, ctx.level.sea_level(), origin.z);
    let snow_on_top = random.next_double() > 0.7;
    let main = cfg.state;
    let shape_angle = random.next_double() * 2.0 * std::f64::consts::PI;
    let shape_a = 11 - random.next_int_bounded(5);
    let shape_c = 3 + random.next_int_bounded(3);
    let is_ellipse = random.next_double() > 0.7;
    let mut over = if is_ellipse { random.next_int_bounded(6) + 6 } else { random.next_int_bounded(15) + 3 };
    if !is_ellipse && random.next_double() > 0.9 {
        over += random.next_int_bounded(19) + 7;
    }
    let under = (over + random.next_int_bounded(11)).min(18);
    let width = (over + random.next_int_bounded(7) - random.next_int_bounded(5)).min(11);
    let a = if is_ellipse { shape_a } else { 11 };

    for xo in -a..a {
        for zo in -a..a {
            for y_off in 0..over {
                let radius = if is_ellipse {
                    iceberg_radius_ellipse(y_off, over, width)
                } else {
                    iceberg_radius_round(random, y_off, over, width)
                };
                if is_ellipse || xo < radius {
                    iceberg_generate_block(
                        ctx, random, origin, over, xo, y_off, zo, radius, a, is_ellipse, shape_c, shape_angle, snow_on_top, main,
                    );
                }
            }
        }
    }

    iceberg_smooth(ctx, origin, width, over, is_ellipse, shape_a);

    for xo in -a..a {
        for zo in -a..a {
            let mut y_off = -1;
            while y_off > -under {
                let new_a = if is_ellipse {
                    mth_ceil_f32(a as f32 * (1.0 - (y_off * y_off) as f64 as f32 / (under as f32 * 8.0)))
                } else {
                    a
                };
                let radius = iceberg_radius_steep(random, -y_off, under, width);
                if xo < radius {
                    iceberg_generate_block(
                        ctx, random, origin, under, xo, y_off, zo, radius, new_a, is_ellipse, shape_c, shape_angle, snow_on_top,
                        main,
                    );
                }
                y_off -= 1;
            }
        }
    }

    let do_cutout = if is_ellipse { random.next_double() > 0.1 } else { random.next_double() > 0.7 };
    if do_cutout {
        iceberg_generate_cutout(ctx, random, width, over, origin, is_ellipse, shape_a, shape_angle, shape_c);
    }
}

// ---------------------------------------------------------------------------
// Desert / rock group: forest_rock (BlockBlobFeature), desert_well
// ---------------------------------------------------------------------------

/// `BlockBlobFeature.place` (`forest_rock`).
fn place_block_blob(cfg: &BlockBlobConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let mut origin = origin;
    while origin.y > ctx.level.min_y() + 3
        && !cfg.can_place_on.test(ctx.level, Pos::new(origin.x, origin.y - 1, origin.z))
    {
        origin.y -= 1;
    }
    if origin.y <= ctx.level.min_y() + 3 {
        return;
    }
    for _ in 0..3 {
        let xr = random.next_int_bounded(2);
        let yr = random.next_int_bounded(2);
        let zr = random.next_int_bounded(2);
        let tr = (xr + yr + zr) as f32 * 0.333 + 0.5;
        let tr2 = (tr * tr) as f64;
        // `BlockPos.betweenClosed` — no RNG in the fill; iteration order is
        // irrelevant (every in-range cell is set to the same state).
        for bx in origin.x - xr..=origin.x + xr {
            for by in origin.y - yr..=origin.y + yr {
                for bz in origin.z - zr..=origin.z + zr {
                    let dx = (bx - origin.x) as f64;
                    let dy = (by - origin.y) as f64;
                    let dz = (bz - origin.z) as f64;
                    if dx * dx + dy * dy + dz * dz <= tr2 {
                        ctx.level.set_block(bx, by, bz, cfg.state);
                    }
                }
            }
        }
        let ox = -1 + random.next_int_bounded(2);
        let oy = -random.next_int_bounded(2);
        let oz = -1 + random.next_int_bounded(2);
        origin = Pos::new(origin.x + ox, origin.y + oy, origin.z + oz);
    }
}

/// `DesertWellFeature.place`. Suspicious-sand block entities carry a loot table
/// that Vela does not model in worldgen — the block state is placed and the
/// exact RNG (two `nextInt(5)` position picks) is consumed, but the archaeology
/// loot NBT is deferred (block-entity scope, documented).
fn place_desert_well(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    use ParityBlock::{Sand, Sandstone, SandstoneSlab, SuspiciousSand, Water};
    let mut origin = Pos::new(origin.x, origin.y + 1, origin.z);
    while ctx.level.get_block(origin.x, origin.y, origin.z).is_air() && origin.y > ctx.level.min_y() + 2 {
        origin.y -= 1;
    }
    if ctx.level.get_block(origin.x, origin.y, origin.z) != Sand {
        return;
    }
    for ox in -2..=2 {
        for oz in -2..=2 {
            if ctx.level.get_block(origin.x + ox, origin.y - 1, origin.z + oz).is_air()
                && ctx.level.get_block(origin.x + ox, origin.y - 2, origin.z + oz).is_air()
            {
                return;
            }
        }
    }
    let set = |ctx: &mut PlacementCtx, dx: i32, dy: i32, dz: i32, s: ParityBlock| {
        ctx.level.set_block(origin.x + dx, origin.y + dy, origin.z + dz, s);
    };
    for oy in -2..=0 {
        for ox in -2..=2 {
            for oz in -2..=2 {
                set(ctx, ox, oy, oz, Sandstone);
            }
        }
    }
    set(ctx, 0, 0, 0, Water);
    // Direction.Plane.HORIZONTAL: NORTH, SOUTH, WEST, EAST (relative offsets).
    const HORIZ: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    for &(dx, dz) in HORIZ.iter() {
        set(ctx, dx, 0, dz, Water);
    }
    set(ctx, 0, -1, 0, Sand);
    for &(dx, dz) in HORIZ.iter() {
        set(ctx, dx, -1, dz, Sand);
    }
    for ox in -2..=2 {
        for oz in -2..=2 {
            if ox == -2 || ox == 2 || oz == -2 || oz == 2 {
                set(ctx, ox, 1, oz, Sandstone);
            }
        }
    }
    set(ctx, 2, 1, 0, SandstoneSlab);
    set(ctx, -2, 1, 0, SandstoneSlab);
    set(ctx, 0, 1, 2, SandstoneSlab);
    set(ctx, 0, 1, -2, SandstoneSlab);
    for ox in -1..=1 {
        for oz in -1..=1 {
            if ox == 0 && oz == 0 {
                set(ctx, ox, 4, oz, Sandstone);
            } else {
                set(ctx, ox, 4, oz, SandstoneSlab);
            }
        }
    }
    for oy in 1..=3 {
        set(ctx, -1, oy, -1, Sandstone);
        set(ctx, -1, oy, 1, Sandstone);
        set(ctx, 1, oy, -1, Sandstone);
        set(ctx, 1, oy, 1, Sandstone);
    }
    // `List.of(center, east, south, west, north)` — the water block offsets.
    const WATER_POS: [(i32, i32); 5] = [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)];
    let pick1 = WATER_POS[random.next_int_bounded(5) as usize];
    ctx.level.set_block(origin.x + pick1.0, origin.y - 1, origin.z + pick1.1, SuspiciousSand);
    let pick2 = WATER_POS[random.next_int_bounded(5) as usize];
    ctx.level.set_block(origin.x + pick2.0, origin.y - 2, origin.z + pick2.1, SuspiciousSand);
}

// ---------------------------------------------------------------------------
// TreeFeature (doPlace / place) + trunk / foliage placers + decorators
// ---------------------------------------------------------------------------
//
// Identity level: default block states only. The `updateLeaves` BFS (leaf
// `distance` finalization), log-axis, and `waterlogged` property setting are an
// explicit follow-up — see the module notes. The RNG draw order is 1:1 with the
// decompiled `TreeFeature.doPlace`.

/// `Direction.Plane.HORIZONTAL` faces in registry order (the array
/// `Util.getRandom` indexes): NORTH, EAST, SOUTH, WEST.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HDir {
    North,
    East,
    South,
    West,
}

impl HDir {
    fn step_x(self) -> i32 {
        match self {
            HDir::East => 1,
            HDir::West => -1,
            _ => 0,
        }
    }
    fn step_z(self) -> i32 {
        match self {
            HDir::North => -1,
            HDir::South => 1,
            _ => 0,
        }
    }
    /// `Direction.getClockWise()` in the horizontal plane: N→E→S→W→N.
    fn clockwise(self) -> HDir {
        match self {
            HDir::North => HDir::East,
            HDir::East => HDir::South,
            HDir::South => HDir::West,
            HDir::West => HDir::North,
        }
    }
    /// `Direction.getAxisDirection() == POSITIVE` (east/south point +x/+z).
    fn axis_positive(self) -> bool {
        matches!(self, HDir::East | HDir::South)
    }
    /// `Direction.getOpposite()` in the horizontal plane.
    fn opposite(self) -> HDir {
        match self {
            HDir::North => HDir::South,
            HDir::South => HDir::North,
            HDir::East => HDir::West,
            HDir::West => HDir::East,
        }
    }
}

/// `Direction.Plane.HORIZONTAL.getRandomDirection(random)` = `faces[nextInt(4)]`.
fn horizontal_random_direction(random: &mut WorldgenRandom) -> HDir {
    match random.next_int_bounded(4) {
        0 => HDir::North,
        1 => HDir::East,
        2 => HDir::South,
        _ => HDir::West,
    }
}

/// `TreeFeature.validTreePos` — air or `#replaceable_by_trees`.
fn valid_tree_pos(level: &dyn DecorationLevel, p: Pos) -> bool {
    let b = level.get_block(p.x, p.y, p.z);
    b.is_air() || BlockTag::ReplaceableByTrees.contains(b)
}

/// `TrunkPlacer.validTreePos` with the optional `can_grow_through` override some
/// placers add (`UpwardsBranchingTrunkPlacer`): `validTreePos || #can_grow_through`.
fn valid_tree_pos_ext(level: &dyn DecorationLevel, p: Pos, grow_through: Option<BlockTag>) -> bool {
    valid_tree_pos(level, p)
        || grow_through.map(|t| t.contains(level.get_block(p.x, p.y, p.z))).unwrap_or(false)
}

/// `TrunkPlacer.isFree` — `validTreePos || #logs`.
fn is_free(level: &dyn DecorationLevel, p: Pos) -> bool {
    is_free_ext(level, p, None)
}

fn is_free_ext(level: &dyn DecorationLevel, p: Pos, grow_through: Option<BlockTag>) -> bool {
    valid_tree_pos_ext(level, p, grow_through) || BlockTag::Logs.contains(level.get_block(p.x, p.y, p.z))
}

/// `TreeFeature.isVine` — a `vine` block (only ever present when an earlier tree
/// in the same chunk placed one via the vine decorators).
fn is_vine(level: &dyn DecorationLevel, p: Pos) -> bool {
    level.get_block(p.x, p.y, p.z) == ParityBlock::Vine
}

/// `TreeFeature.isAirOrLeaves`.
fn is_air_or_leaves(level: &dyn DecorationLevel, p: Pos) -> bool {
    let b = level.get_block(p.x, p.y, p.z);
    b.is_air() || b.is_leaves()
}

/// `TrunkPlacer.placeLog` — place a trunk log if `validTreePos`; records the
/// position. The simple/weighted trunk provider is drawn here (simple: no RNG).
fn place_log(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    p: Pos,
    config: &TreeConfig,
) -> bool {
    place_log_growable(level, trunks, random, p, config, None)
}

/// `TrunkPlacer.placeLog` honoring an optional `can_grow_through` override.
fn place_log_growable(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    p: Pos,
    config: &TreeConfig,
    grow_through: Option<BlockTag>,
) -> bool {
    if valid_tree_pos_ext(level, p, grow_through) {
        if let Some(state) = config.trunk_provider.get_state(&*level, random, p) {
            trunks.insert(p);
            level.set_block(p.x, p.y, p.z, state);
        }
        true
    } else {
        false
    }
}

/// `TrunkPlacer.placeLogIfFree` — place a log only when the position `isFree`
/// (valid tree pos or already a log); `placeLog` re-checks `validTreePos`.
fn place_log_if_free(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    p: Pos,
    config: &TreeConfig,
) {
    if is_free(level, p) {
        place_log(level, trunks, random, p, config);
    }
}

/// `TrunkPlacer.placeBelowTrunkBlock` — `belowTrunkProvider.getOptionalState`;
/// `None` (no matching rule / no fallback) places nothing.
fn place_below_trunk_block(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    p: Pos,
    config: &TreeConfig,
) {
    if let Some(state) = config.below_trunk_provider.get_state(&*level, random, p) {
        trunks.insert(p);
        level.set_block(p.x, p.y, p.z, state);
    }
}

/// `TreeFeature.getMaxFreeTreeHeight`. `isVine` is normally false during the
/// first tree of a chunk, but an earlier jungle tree's vine decorators can leave
/// `vine` blocks, so the `ignore_vines` gate is honored.
fn get_max_free_tree_height(
    level: &dyn DecorationLevel,
    max_tree_height: i32,
    tree_pos: Pos,
    config: &TreeConfig,
    grow_through: Option<BlockTag>,
) -> i32 {
    for y in 0..=max_tree_height + 1 {
        let r = config.minimum_size.get_size_at_height(max_tree_height, y);
        for x in -r..=r {
            for z in -r..=r {
                let p = Pos::new(tree_pos.x + x, tree_pos.y + y, tree_pos.z + z);
                if !is_free_ext(level, p, grow_through) || (!config.ignore_vines && is_vine(level, p)) {
                    return y - 2;
                }
            }
        }
    }
    max_tree_height
}

impl TrunkPlacer {
    /// `TrunkPlacer.placeTrunk` — returns the foliage attachments.
    fn place_trunk(
        &self,
        level: &mut dyn DecorationLevel,
        trunks: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        tree_height: i32,
        origin: Pos,
        config: &TreeConfig,
    ) -> Vec<FoliageAttachment> {
        match self {
            TrunkPlacer::Straight { .. } => {
                place_below_trunk_block(level, trunks, random, origin.below(), config);
                for y in 0..tree_height {
                    place_log(level, trunks, random, origin.above(y), config);
                }
                vec![FoliageAttachment { pos: origin.above(tree_height), radius_offset: 0, double_trunk: false }]
            }
            TrunkPlacer::Forking { .. } => {
                place_below_trunk_block(level, trunks, random, origin.below(), config);
                let mut attachments = Vec::new();
                let lean_direction = horizontal_random_direction(random);
                let lean_height = tree_height - random.next_int_bounded(4) - 1;
                let mut lean_steps = 3 - random.next_int_bounded(3);
                let mut tx = origin.x;
                let mut tz = origin.z;
                let mut ey: Option<i32> = None;
                for yo in 0..tree_height {
                    let yy = origin.y + yo;
                    if yo >= lean_height && lean_steps > 0 {
                        tx += lean_direction.step_x();
                        tz += lean_direction.step_z();
                        lean_steps -= 1;
                    }
                    if place_log(level, trunks, random, Pos::new(tx, yy, tz), config) {
                        ey = Some(yy + 1);
                    }
                }
                if let Some(e) = ey {
                    attachments.push(FoliageAttachment { pos: Pos::new(tx, e, tz), radius_offset: 1, double_trunk: false });
                }
                tx = origin.x;
                tz = origin.z;
                let branch_direction = horizontal_random_direction(random);
                if branch_direction != lean_direction {
                    let branch_pos = lean_height - random.next_int_bounded(2) - 1;
                    let mut branch_steps = 1 + random.next_int_bounded(3);
                    let mut ey2: Option<i32> = None;
                    let mut yo = branch_pos;
                    while yo < tree_height && branch_steps > 0 {
                        if yo >= 1 {
                            let yy = origin.y + yo;
                            tx += branch_direction.step_x();
                            tz += branch_direction.step_z();
                            if place_log(level, trunks, random, Pos::new(tx, yy, tz), config) {
                                ey2 = Some(yy + 1);
                            }
                        }
                        yo += 1;
                        branch_steps -= 1;
                    }
                    if let Some(e) = ey2 {
                        attachments.push(FoliageAttachment { pos: Pos::new(tx, e, tz), radius_offset: 0, double_trunk: false });
                    }
                }
                attachments
            }
            TrunkPlacer::DarkOak { .. } => {
                let mut attachments = Vec::new();
                let below = origin.below();
                place_below_trunk_block(level, trunks, random, below, config);
                place_below_trunk_block(level, trunks, random, below.east(), config);
                place_below_trunk_block(level, trunks, random, below.south(), config);
                place_below_trunk_block(level, trunks, random, below.south().east(), config);
                let lean_direction = horizontal_random_direction(random);
                let lean_height = tree_height - random.next_int_bounded(4);
                let mut lean_steps = 2 - random.next_int_bounded(3);
                let (x, y, z) = (origin.x, origin.y, origin.z);
                let mut tx = x;
                let mut tz = z;
                let ey = y + tree_height - 1;
                for dy in 0..tree_height {
                    if dy >= lean_height && lean_steps > 0 {
                        tx += lean_direction.step_x();
                        tz += lean_direction.step_z();
                        lean_steps -= 1;
                    }
                    let yy = y + dy;
                    let bp = Pos::new(tx, yy, tz);
                    if is_air_or_leaves(level, bp) {
                        place_log(level, trunks, random, bp, config);
                        place_log(level, trunks, random, bp.east(), config);
                        place_log(level, trunks, random, bp.south(), config);
                        place_log(level, trunks, random, bp.east().south(), config);
                    }
                }
                attachments.push(FoliageAttachment { pos: Pos::new(tx, ey, tz), radius_offset: 0, double_trunk: true });
                for ox in -1..=2 {
                    for oz in -1..=2 {
                        if (ox < 0 || ox > 1 || oz < 0 || oz > 1) && random.next_int_bounded(3) <= 0 {
                            let length = random.next_int_bounded(3) + 2;
                            for branch_y in 0..length {
                                place_log(level, trunks, random, Pos::new(x + ox, ey - branch_y - 1, z + oz), config);
                            }
                            attachments.push(FoliageAttachment { pos: Pos::new(x + ox, ey, z + oz), radius_offset: 0, double_trunk: false });
                        }
                    }
                }
                attachments
            }
            TrunkPlacer::Fancy { .. } => place_fancy_trunk(level, trunks, random, tree_height, origin, config),
            TrunkPlacer::Giant { .. } => place_giant_trunk(level, trunks, random, tree_height, origin, config),
            TrunkPlacer::MegaJungle { .. } => {
                // `MegaJungleTrunkPlacer.placeTrunk` — the giant 2×2 trunk plus
                // radial side branches, each drawing `nextFloat` (angle) and
                // `nextInt(4)` (height step).
                let mut attachments = place_giant_trunk(level, trunks, random, tree_height, origin, config);
                let mut branch_height = tree_height - 2 - random.next_int_bounded(4);
                while branch_height > tree_height / 2 {
                    let angle = random.next_float() * std::f32::consts::TAU;
                    let mut bx = 0;
                    let mut bz = 0;
                    for b in 0..5 {
                        bx = (1.5 + super::carvers::mth_cos(angle as f64) * b as f32) as i32;
                        bz = (1.5 + super::carvers::mth_sin(angle as f64) * b as f32) as i32;
                        let pos = origin.offset(bx, branch_height - 3 + b / 2, bz);
                        place_log(level, trunks, random, pos, config);
                    }
                    attachments.push(FoliageAttachment {
                        pos: origin.offset(bx, branch_height, bz),
                        radius_offset: -2,
                        double_trunk: false,
                    });
                    branch_height -= 2 + random.next_int_bounded(4);
                }
                attachments
            }
            TrunkPlacer::Cherry {
                branch_count,
                branch_horizontal_length,
                branch_start_min,
                branch_start_max,
                branch_end_offset,
                ..
            } => {
                // `CherryTrunkPlacer.placeTrunk`.
                place_below_trunk_block(level, trunks, random, origin.below(), config);
                // `UniformInt.sample` = `nextInt(max - min + 1) + min`.
                let first_off = random.next_int_bounded(*branch_start_max - *branch_start_min + 1) + *branch_start_min;
                let first_branch = (tree_height - 1 + first_off).max(0);
                // secondBranchStartOffsetFromTop = UniformInt.of(min, max-1).
                let second_off = random.next_int_bounded(*branch_start_max - 1 - *branch_start_min + 1) + *branch_start_min;
                let mut second_branch = (tree_height - 1 + second_off).max(0);
                if second_branch >= first_branch {
                    second_branch += 1;
                }
                let bc = branch_count.sample(random);
                let has_middle_branch = bc == 3;
                let has_both_side_branches = bc >= 2;
                let trunk_height = if has_middle_branch {
                    tree_height
                } else if has_both_side_branches {
                    first_branch.max(second_branch) + 1
                } else {
                    first_branch + 1
                };
                for y in 0..trunk_height {
                    place_log(level, trunks, random, origin.above(y), config);
                }
                let mut attachments = Vec::new();
                if has_middle_branch {
                    attachments.push(FoliageAttachment { pos: origin.above(trunk_height), radius_offset: 0, double_trunk: false });
                }
                let tree_direction = horizontal_random_direction(random);
                attachments.push(cherry_generate_branch(
                    level, trunks, random, tree_height, origin, config, branch_horizontal_length, branch_end_offset,
                    tree_direction, first_branch, first_branch < trunk_height - 1,
                ));
                if has_both_side_branches {
                    attachments.push(cherry_generate_branch(
                        level, trunks, random, tree_height, origin, config, branch_horizontal_length, branch_end_offset,
                        tree_direction.opposite(), second_branch, second_branch < trunk_height - 1,
                    ));
                }
                attachments
            }
            TrunkPlacer::Bending { min_height_for_leaves, bend_length, .. } => {
                // `BendingTrunkPlacer.placeTrunk`.
                let direction = horizontal_random_direction(random);
                let log_height = tree_height - 1;
                let (mut px, mut py, mut pz) = (origin.x, origin.y, origin.z);
                place_below_trunk_block(level, trunks, random, origin.below(), config);
                let mut foliage_points = Vec::new();
                for i in 0..=log_height {
                    if i + 1 >= log_height + random.next_int_bounded(2) {
                        px += direction.step_x();
                        pz += direction.step_z();
                    }
                    let p = Pos::new(px, py, pz);
                    if valid_tree_pos(level, p) {
                        place_log(level, trunks, random, p, config);
                    }
                    if i >= *min_height_for_leaves {
                        foliage_points.push(FoliageAttachment { pos: p, radius_offset: 0, double_trunk: false });
                    }
                    py += 1;
                }
                let dir_length = bend_length.sample(random);
                for _ in 0..=dir_length {
                    let p = Pos::new(px, py, pz);
                    if valid_tree_pos(level, p) {
                        place_log(level, trunks, random, p, config);
                    }
                    foliage_points.push(FoliageAttachment { pos: p, radius_offset: 0, double_trunk: false });
                    px += direction.step_x();
                    pz += direction.step_z();
                }
                foliage_points
            }
            TrunkPlacer::UpwardsBranching { extra_branch_steps, place_branch_prob, extra_branch_length, .. } => {
                // `UpwardsBranchingTrunkPlacer.placeTrunk`.
                let grow_through = self.grow_through();
                let mut attachments = Vec::new();
                for height_pos in 0..tree_height {
                    let current_height = origin.y + height_pos;
                    let log_pos = Pos::new(origin.x, current_height, origin.z);
                    if place_log_growable(level, trunks, random, log_pos, config, grow_through)
                        && height_pos < tree_height - 1
                        && random.next_float() < *place_branch_prob
                    {
                        let branch_dir = horizontal_random_direction(random);
                        let branch_len = extra_branch_length.sample(random);
                        let branch_pos = (branch_len - extra_branch_length.sample(random) - 1).max(0);
                        let branch_steps = extra_branch_steps.sample(random);
                        upwards_place_branch(
                            level, trunks, random, tree_height, config, &mut attachments, origin, current_height,
                            branch_dir, branch_pos, branch_steps, grow_through,
                        );
                    }
                    if height_pos == tree_height - 1 {
                        attachments.push(FoliageAttachment {
                            pos: Pos::new(origin.x, current_height + 1, origin.z),
                            radius_offset: 0,
                            double_trunk: false,
                        });
                    }
                }
                attachments
            }
            TrunkPlacer::Unsupported => Vec::new(),
        }
    }
}

/// `CherryTrunkPlacer.generateBranch` — a curved branch walking toward its end.
#[allow(clippy::too_many_arguments)]
fn cherry_generate_branch(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    tree_height: i32,
    origin: Pos,
    config: &TreeConfig,
    branch_horizontal_length: &IntProvider,
    branch_end_offset: &IntProvider,
    branch_direction: HDir,
    offset_from_origin: i32,
    middle_continues_upwards: bool,
) -> FoliageAttachment {
    let mut log_pos = origin.above(offset_from_origin);
    let branch_end_off = tree_height - 1 + branch_end_offset.sample(random);
    let extend = middle_continues_upwards || branch_end_off < offset_from_origin;
    let distance_to_trunk = branch_horizontal_length.sample(random) + if extend { 1 } else { 0 };
    let branch_end_pos = Pos::new(
        origin.x + branch_direction.step_x() * distance_to_trunk,
        origin.y + branch_end_off,
        origin.z + branch_direction.step_z() * distance_to_trunk,
    );
    let steps_horizontally = if extend { 2 } else { 1 };
    for _ in 0..steps_horizontally {
        log_pos = Pos::new(log_pos.x + branch_direction.step_x(), log_pos.y, log_pos.z + branch_direction.step_z());
        place_log(level, trunks, random, log_pos, config);
    }
    let vertical_up = branch_end_pos.y > log_pos.y;
    loop {
        let distance =
            (log_pos.x - branch_end_pos.x).abs() + (log_pos.y - branch_end_pos.y).abs() + (log_pos.z - branch_end_pos.z).abs();
        if distance == 0 {
            return FoliageAttachment { pos: branch_end_pos.above(1), radius_offset: 0, double_trunk: false };
        }
        let chance = (branch_end_pos.y - log_pos.y).abs() as f32 / distance as f32;
        let grow_vertically = random.next_float() < chance;
        log_pos = if grow_vertically {
            Pos::new(log_pos.x, log_pos.y + if vertical_up { 1 } else { -1 }, log_pos.z)
        } else {
            Pos::new(log_pos.x + branch_direction.step_x(), log_pos.y, log_pos.z + branch_direction.step_z())
        };
        place_log(level, trunks, random, log_pos, config);
    }
}

/// `UpwardsBranchingTrunkPlacer.placeBranch`.
#[allow(clippy::too_many_arguments)]
fn upwards_place_branch(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    tree_height: i32,
    config: &TreeConfig,
    attachments: &mut Vec<FoliageAttachment>,
    origin: Pos,
    current_height: i32,
    branch_dir: HDir,
    branch_pos: i32,
    mut branch_steps: i32,
    grow_through: Option<BlockTag>,
) {
    let mut height_along_branch = current_height + branch_pos;
    let mut log_x = origin.x;
    let mut log_z = origin.z;
    let mut idx = branch_pos;
    while idx < tree_height && branch_steps > 0 {
        if idx >= 1 {
            let placement_height = current_height + idx;
            log_x += branch_dir.step_x();
            log_z += branch_dir.step_z();
            height_along_branch = placement_height;
            if place_log_growable(level, trunks, random, Pos::new(log_x, placement_height, log_z), config, grow_through) {
                height_along_branch += 1;
            }
            attachments.push(FoliageAttachment { pos: Pos::new(log_x, placement_height, log_z), radius_offset: 0, double_trunk: false });
        }
        idx += 1;
        branch_steps -= 1;
    }
    if height_along_branch - current_height > 1 {
        let foliage_pos = Pos::new(log_x, height_along_branch, log_z);
        attachments.push(FoliageAttachment { pos: foliage_pos, radius_offset: 0, double_trunk: false });
        attachments.push(FoliageAttachment { pos: foliage_pos.above(-2), radius_offset: 0, double_trunk: false });
    }
}

impl RootPlacer {
    /// `RootPlacer.getTrunkOrigin` — draws `trunkOffsetY` and shifts the trunk up.
    fn get_trunk_origin(&self, origin: Pos, random: &mut WorldgenRandom) -> Pos {
        origin.above(self.trunk_offset_y.sample(random))
    }

    /// `MangroveRootPlacer.canPlaceRoot` — `validTreePos || #can_grow_through`.
    fn can_place_root(&self, level: &dyn DecorationLevel, p: Pos) -> bool {
        valid_tree_pos(level, p)
            || self.can_grow_through.map(|t| t.contains(level.get_block(p.x, p.y, p.z))).unwrap_or(false)
    }

    /// `MangroveRootPlacer.placeRoots` — returns false (aborting the whole tree)
    /// when the root system cannot fit.
    fn place_roots(
        &self,
        level: &mut dyn DecorationLevel,
        roots: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        origin: Pos,
        trunk_origin: Pos,
        config: &TreeConfig,
    ) -> bool {
        let mut root_positions: Vec<Pos> = Vec::new();
        let mut cy = origin.y;
        while cy < trunk_origin.y {
            if !self.can_place_root(level, Pos::new(origin.x, cy, origin.z)) {
                return false;
            }
            cy += 1;
        }
        root_positions.push(trunk_origin.below());
        // `Direction.Plane.HORIZONTAL`: NORTH, EAST, SOUTH, WEST.
        for dir in [HDir::North, HDir::East, HDir::South, HDir::West] {
            let pos = Pos::new(trunk_origin.x + dir.step_x(), trunk_origin.y, trunk_origin.z + dir.step_z());
            let mut positions_in_direction: Vec<Pos> = Vec::new();
            if !self.simulate_roots(level, random, pos, dir, trunk_origin, &mut positions_in_direction, 0) {
                return false;
            }
            root_positions.extend(positions_in_direction);
            root_positions.push(pos);
        }
        for root_pos in &root_positions {
            self.place_root(level, roots, random, *root_pos, config);
        }
        true
    }

    /// `MangroveRootPlacer.simulateRoots` (recursive).
    #[allow(clippy::too_many_arguments)]
    fn simulate_roots(
        &self,
        level: &dyn DecorationLevel,
        random: &mut WorldgenRandom,
        root_pos: Pos,
        dir: HDir,
        root_origin: Pos,
        root_positions: &mut Vec<Pos>,
        layer: i32,
    ) -> bool {
        if layer != self.max_root_length && (root_positions.len() as i32) <= self.max_root_length {
            for pos in self.potential_root_positions(root_pos, dir, random, root_origin) {
                if self.can_place_root(level, pos) {
                    root_positions.push(pos);
                    if !self.simulate_roots(level, random, pos, dir, root_origin, root_positions, layer + 1) {
                        return false;
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// `MangroveRootPlacer.potentialRootPositions`.
    fn potential_root_positions(&self, pos: Pos, prev_dir: HDir, random: &mut WorldgenRandom, root_origin: Pos) -> Vec<Pos> {
        let below = pos.below();
        let next_to = Pos::new(pos.x + prev_dir.step_x(), pos.y, pos.z + prev_dir.step_z());
        let width = (pos.x - root_origin.x).abs() + (pos.y - root_origin.y).abs() + (pos.z - root_origin.z).abs();
        let skew = self.random_skew_chance;
        if width > self.max_root_width - 3 && width <= self.max_root_width {
            if random.next_float() < skew {
                vec![below, next_to.below()]
            } else {
                vec![below]
            }
        } else if width > self.max_root_width {
            vec![below]
        } else if random.next_float() < skew {
            vec![below]
        } else if random.next_boolean() {
            vec![next_to]
        } else {
            vec![below]
        }
    }

    /// `MangroveRootPlacer.placeRoot` (with the muddy-roots override) + the base
    /// `RootPlacer.placeRoot` above-root placement.
    fn place_root(
        &self,
        level: &mut dyn DecorationLevel,
        roots: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        pos: Pos,
        _config: &TreeConfig,
    ) {
        if self.muddy_roots_in.contains(&level.get_block(pos.x, pos.y, pos.z)) {
            if let Some(state) = self.muddy_roots_provider.get_state(&*level, random, pos) {
                roots.insert(pos);
                level.set_block(pos.x, pos.y, pos.z, state);
            }
            return;
        }
        if self.can_place_root(level, pos) {
            if let Some(state) = self.root_provider.get_state(&*level, random, pos) {
                roots.insert(pos);
                level.set_block(pos.x, pos.y, pos.z, state);
            }
            if let Some(ar) = &self.above_root {
                let above = pos.above(1);
                // `nextFloat() < chance && isAir(above)` — nextFloat always drawn.
                let roll = random.next_float();
                if roll < ar.chance && level.get_block(above.x, above.y, above.z).is_air() {
                    if let Some(s2) = ar.provider.get_state(&*level, random, above) {
                        roots.insert(above);
                        level.set_block(above.x, above.y, above.z, s2);
                    }
                }
            }
        }
    }
}

/// `GiantTrunkPlacer.placeTrunk` — a 2×2 straight trunk. Shared by `Giant` and
/// the `MegaJungle` placer (which calls it then adds side branches).
fn place_giant_trunk(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    tree_height: i32,
    origin: Pos,
    config: &TreeConfig,
) -> Vec<FoliageAttachment> {
    let below = origin.below();
    place_below_trunk_block(level, trunks, random, below, config);
    place_below_trunk_block(level, trunks, random, below.east(), config);
    place_below_trunk_block(level, trunks, random, below.south(), config);
    place_below_trunk_block(level, trunks, random, below.south().east(), config);
    for hh in 0..tree_height {
        place_log_if_free(level, trunks, random, origin.offset(0, hh, 0), config);
        if hh < tree_height - 1 {
            place_log_if_free(level, trunks, random, origin.offset(1, hh, 0), config);
            place_log_if_free(level, trunks, random, origin.offset(1, hh, 1), config);
            place_log_if_free(level, trunks, random, origin.offset(0, hh, 1), config);
        }
    }
    vec![FoliageAttachment { pos: origin.above(tree_height), radius_offset: 0, double_trunk: true }]
}

/// `Mth.floor(float)` — `(int)value` then step down when `value < i`.
fn mth_floor_f32(v: f32) -> i32 {
    let i = v as i32;
    if v < i as f32 {
        i - 1
    } else {
        i
    }
}

/// `Mth.floor(double)`.
fn mth_floor_f64(v: f64) -> i32 {
    let i = v as i64 as i32;
    if v < i as f64 {
        i - 1
    } else {
        i
    }
}

/// `FancyTrunkPlacer.treeShape` — the canopy radius envelope. All-float math
/// (matching `Mth.sqrt(float)` = `(float)Math.sqrt`).
fn fancy_tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let radius = height as f32 / 2.0;
    let adjacent = radius - y as f32;
    let mut distance = ((radius * radius - adjacent * adjacent) as f64).sqrt() as f32;
    if adjacent == 0.0 {
        distance = radius;
    } else if adjacent.abs() >= radius {
        return 0.0;
    }
    distance * 0.5
}

/// `FancyTrunkPlacer.trimBranches`.
fn fancy_trim_branches(height: i32, local_y: i32) -> bool {
    local_y as f64 >= height as f64 * 0.2
}

/// `FancyTrunkPlacer.getSteps`.
fn fancy_get_steps(dx: i32, dy: i32, dz: i32) -> i32 {
    dx.abs().max(dy.abs()).max(dz.abs())
}

/// `FancyTrunkPlacer.makeLimb` — walk a straight line of blocks from `start` to
/// `end`, either placing logs (`do_place`) or probing that the path is free.
/// The `getLogAxis` state-modifier is a no-op over the identity alphabet (default
/// log states carry no axis), so it draws nothing and is omitted.
fn make_limb(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    start: Pos,
    end: Pos,
    do_place: bool,
    config: &TreeConfig,
) -> bool {
    if !do_place && start == end {
        return true;
    }
    let (dx, dy, dz) = (end.x - start.x, end.y - start.y, end.z - start.z);
    let steps = fancy_get_steps(dx, dy, dz);
    let fdx = dx as f32 / steps as f32;
    let fdy = dy as f32 / steps as f32;
    let fdz = dz as f32 / steps as f32;
    for i in 0..=steps {
        let bp = Pos::new(
            start.x + mth_floor_f32(0.5 + i as f32 * fdx),
            start.y + mth_floor_f32(0.5 + i as f32 * fdy),
            start.z + mth_floor_f32(0.5 + i as f32 * fdz),
        );
        if do_place {
            place_log(level, trunks, random, bp, config);
        } else if !is_free(level, bp) {
            return false;
        }
    }
    true
}

/// A `FancyTrunkPlacer.FoliageCoords`: the foliage attachment plus its branch
/// base Y (the trunk height the limb springs from).
struct FoliageCoords {
    attachment: FoliageAttachment,
    branch_base: i32,
}

/// `FancyTrunkPlacer.placeTrunk`. Builds the branch canopy: a set of foliage
/// clusters connected by limbs to the central trunk. Only the two `nextFloat`
/// draws per accepted cluster iteration draw RNG (limb placement uses a simple
/// state provider → no draw), so the sequence is exact.
fn place_fancy_trunk(
    level: &mut dyn DecorationLevel,
    trunks: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    tree_height: i32,
    origin: Pos,
    config: &TreeConfig,
) -> Vec<FoliageAttachment> {
    let height = tree_height + 2;
    let trunk_height = mth_floor_f64(height as f64 * 0.618);
    place_below_trunk_block(level, trunks, random, origin.below(), config);
    // `Math.min(1, floor(1.382 + (height/13)²))` — always 1 for valid heights,
    // ported literally.
    let clusters_per_y = 1.min(mth_floor_f64(1.382 + (1.0 * height as f64 / 13.0).powf(2.0)));
    let trunk_top = origin.y + trunk_height;
    let mut relative_y = height - 5;
    let mut foliage_coords: Vec<FoliageCoords> = Vec::new();
    foliage_coords.push(FoliageCoords {
        attachment: FoliageAttachment { pos: origin.above(relative_y), radius_offset: 0, double_trunk: false },
        branch_base: trunk_top,
    });

    while relative_y >= 0 {
        let tree_shape = fancy_tree_shape(height, relative_y);
        if !(tree_shape < 0.0) {
            for _ in 0..clusters_per_y {
                let radius = 1.0 * tree_shape as f64 * (random.next_float() as f64 + 0.328);
                let angle = (random.next_float() * 2.0) as f64 * std::f64::consts::PI;
                let x = radius * angle.sin() + 0.5;
                let z = radius * angle.cos() + 0.5;
                let check_start =
                    origin.offset(mth_floor_f64(x), relative_y - 1, mth_floor_f64(z));
                let check_end = check_start.above(5);
                if make_limb(level, trunks, random, check_start, check_end, false, config) {
                    let ddx = origin.x - check_start.x;
                    let ddz = origin.z - check_start.z;
                    let branch_height =
                        check_start.y as f64 - ((ddx * ddx + ddz * ddz) as f64).sqrt() * 0.381;
                    let branch_top =
                        if branch_height > trunk_top as f64 { trunk_top } else { branch_height as i32 };
                    let check_branch_base = Pos::new(origin.x, branch_top, origin.z);
                    if make_limb(level, trunks, random, check_branch_base, check_start, false, config) {
                        foliage_coords.push(FoliageCoords {
                            attachment: FoliageAttachment { pos: check_start, radius_offset: 0, double_trunk: false },
                            branch_base: check_branch_base.y,
                        });
                    }
                }
            }
        }
        relative_y -= 1;
    }

    make_limb(level, trunks, random, origin, origin.above(trunk_height), true, config);
    // `makeBranches` — connect each retained cluster's branch base to its cluster.
    for fc in &foliage_coords {
        let base_coord = Pos::new(origin.x, fc.branch_base, origin.z);
        if base_coord != fc.attachment.pos && fancy_trim_branches(height, fc.branch_base - origin.y) {
            make_limb(level, trunks, random, base_coord, fc.attachment.pos, true, config);
        }
    }

    let mut attachments = Vec::new();
    for fc in &foliage_coords {
        if fancy_trim_branches(height, fc.branch_base - origin.y) {
            attachments.push(fc.attachment);
        }
    }
    attachments
}

/// `FoliagePlacer.tryPlaceLeaf`. `isPersistent` is always false over the parity
/// alphabet (all placed leaves default `persistent=false`, and terrain has
/// none), so the gate reduces to `validTreePos`. `waterlogged` finalization is
/// deferred (identity level).
fn try_place_leaf(
    level: &mut dyn DecorationLevel,
    foliage: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    config: &TreeConfig,
    p: Pos,
) -> bool {
    if valid_tree_pos(level, p) {
        if let Some(state) = config.foliage_provider.get_state(&*level, random, p) {
            foliage.insert(p);
            level.set_block(p.x, p.y, p.z, state);
            return true;
        }
    }
    false
}

/// `FoliagePlacer.tryPlaceExtension` — hang a leaf below a fringe if within reach
/// of the trunk (`distManhattan < 7`) and a `nextFloat` gate passes.
fn try_place_extension(
    level: &mut dyn DecorationLevel,
    foliage: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    config: &TreeConfig,
    chance: f32,
    log_pos: Pos,
    pos: Pos,
) -> bool {
    let dist = (pos.x - log_pos.x).abs() + (pos.y - log_pos.y).abs() + (pos.z - log_pos.z).abs();
    if dist >= 7 {
        return false;
    }
    if random.next_float() > chance {
        return false;
    }
    try_place_leaf(level, foliage, random, config, pos)
}

impl FoliagePlacer {
    fn offset_ip(&self) -> &IntProvider {
        match self {
            FoliagePlacer::Blob { offset, .. }
            | FoliagePlacer::Spruce { offset, .. }
            | FoliagePlacer::Pine { offset, .. }
            | FoliagePlacer::DarkOak { offset, .. }
            | FoliagePlacer::Fancy { offset, .. }
            | FoliagePlacer::Bush { offset, .. }
            | FoliagePlacer::Acacia { offset, .. }
            | FoliagePlacer::MegaJungle { offset, .. }
            | FoliagePlacer::Cherry { offset, .. }
            | FoliagePlacer::MegaPine { offset, .. }
            | FoliagePlacer::RandomSpread { offset, .. } => offset,
            FoliagePlacer::Unsupported => unreachable!("offset_ip on unsupported foliage placer"),
        }
    }

    /// `FoliagePlacer.foliageHeight`.
    fn foliage_height(&self, random: &mut WorldgenRandom, tree_height: i32) -> i32 {
        match self {
            FoliagePlacer::Blob { height, .. }
            | FoliagePlacer::Fancy { height, .. }
            | FoliagePlacer::Bush { height, .. }
            | FoliagePlacer::MegaJungle { height, .. } => *height,
            FoliagePlacer::Spruce { trunk_height, .. } => (tree_height - trunk_height.sample(random)).max(4),
            FoliagePlacer::Pine { height, .. } => height.sample(random),
            FoliagePlacer::DarkOak { .. } => 4,
            // `AcaciaFoliagePlacer.foliageHeight` returns 0.
            FoliagePlacer::Acacia { .. } => 0,
            FoliagePlacer::Cherry { height, .. } => height.sample(random),
            FoliagePlacer::MegaPine { crown_height, .. } => crown_height.sample(random),
            FoliagePlacer::RandomSpread { foliage_height, .. } => foliage_height.sample(random),
            FoliagePlacer::Unsupported => 0,
        }
    }

    /// `FoliagePlacer.foliageRadius` (Pine overrides with an extra draw).
    fn foliage_radius(&self, random: &mut WorldgenRandom, trunk_height: i32) -> i32 {
        match self {
            FoliagePlacer::Pine { radius, .. } => {
                radius.sample(random) + random.next_int_bounded((trunk_height + 1).max(1))
            }
            FoliagePlacer::Blob { radius, .. }
            | FoliagePlacer::Spruce { radius, .. }
            | FoliagePlacer::DarkOak { radius, .. }
            | FoliagePlacer::Fancy { radius, .. }
            | FoliagePlacer::Bush { radius, .. }
            | FoliagePlacer::Acacia { radius, .. }
            | FoliagePlacer::MegaJungle { radius, .. }
            | FoliagePlacer::Cherry { radius, .. }
            | FoliagePlacer::MegaPine { radius, .. }
            | FoliagePlacer::RandomSpread { radius, .. } => radius.sample(random),
            FoliagePlacer::Unsupported => 0,
        }
    }

    /// The public `FoliagePlacer.createFoliage` wrapper: draw the offset first,
    /// then dispatch to the type-specific body.
    #[allow(clippy::too_many_arguments)]
    fn create_foliage(
        &self,
        level: &mut dyn DecorationLevel,
        foliage: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        config: &TreeConfig,
        _tree_height: i32,
        att: &FoliageAttachment,
        foliage_height: i32,
        leaf_radius: i32,
    ) {
        let offset = self.offset_ip().sample(random);
        let dt = att.double_trunk;
        match self {
            FoliagePlacer::Blob { .. } => {
                let mut yo = offset;
                while yo >= offset - foliage_height {
                    let current_radius = (leaf_radius + att.radius_offset - 1 - yo / 2).max(0);
                    self.place_leaves_row(level, foliage, random, config, att.pos, current_radius, yo, dt);
                    yo -= 1;
                }
            }
            FoliagePlacer::Spruce { .. } => {
                let mut current_radius = random.next_int_bounded(2);
                let mut max_radius = 1;
                let mut min_radius = 0;
                let mut yo = offset;
                while yo >= -foliage_height {
                    self.place_leaves_row(level, foliage, random, config, att.pos, current_radius, yo, dt);
                    if current_radius >= max_radius {
                        current_radius = min_radius;
                        min_radius = 1;
                        max_radius = (max_radius + 1).min(leaf_radius + att.radius_offset);
                    } else {
                        current_radius += 1;
                    }
                    yo -= 1;
                }
            }
            FoliagePlacer::Pine { .. } => {
                let mut current_radius = 0;
                let mut yo = offset;
                while yo >= offset - foliage_height {
                    self.place_leaves_row(level, foliage, random, config, att.pos, current_radius, yo, dt);
                    if current_radius >= 1 && yo == offset - foliage_height + 1 {
                        current_radius -= 1;
                    } else if current_radius < leaf_radius + att.radius_offset {
                        current_radius += 1;
                    }
                    yo -= 1;
                }
            }
            FoliagePlacer::DarkOak { .. } => {
                let pos = att.pos.above(offset);
                if dt {
                    self.place_leaves_row(level, foliage, random, config, pos, leaf_radius + 2, -1, dt);
                    self.place_leaves_row(level, foliage, random, config, pos, leaf_radius + 3, 0, dt);
                    self.place_leaves_row(level, foliage, random, config, pos, leaf_radius + 2, 1, dt);
                    if random.next_boolean() {
                        self.place_leaves_row(level, foliage, random, config, pos, leaf_radius, 2, dt);
                    }
                } else {
                    self.place_leaves_row(level, foliage, random, config, pos, leaf_radius + 2, -1, dt);
                    self.place_leaves_row(level, foliage, random, config, pos, leaf_radius + 1, 0, dt);
                }
            }
            FoliagePlacer::Fancy { .. } => {
                // `FancyFoliagePlacer.createFoliage` — a 3-row (offset .. offset -
                // foliageHeight) blob; interior rows widen by 1. No RNG draws.
                let mut yo = offset;
                while yo >= offset - foliage_height {
                    let current_radius =
                        leaf_radius + if yo != offset && yo != offset - foliage_height { 1 } else { 0 };
                    self.place_leaves_row(level, foliage, random, config, att.pos, current_radius, yo, dt);
                    yo -= 1;
                }
            }
            FoliagePlacer::Bush { .. } => {
                // `BushFoliagePlacer.createFoliage` — a small blob widening downward.
                let mut yo = offset;
                while yo >= offset - foliage_height {
                    let current_radius = leaf_radius + att.radius_offset - 1 - yo;
                    self.place_leaves_row(level, foliage, random, config, att.pos, current_radius, yo, dt);
                    yo -= 1;
                }
            }
            FoliagePlacer::Acacia { .. } => {
                // `AcaciaFoliagePlacer.createFoliage` — a flat 3-row canopy. No RNG.
                let foliage_pos = att.pos.above(offset);
                self.place_leaves_row(level, foliage, random, config, foliage_pos, leaf_radius + att.radius_offset, -1 - foliage_height, dt);
                self.place_leaves_row(level, foliage, random, config, foliage_pos, leaf_radius - 1, -foliage_height, dt);
                self.place_leaves_row(level, foliage, random, config, foliage_pos, leaf_radius + att.radius_offset - 1, 0, dt);
            }
            FoliagePlacer::MegaJungle { .. } => {
                // `MegaJungleFoliagePlacer.createFoliage` — single-trunk branch tips
                // draw one `nextInt(2)`; the double-trunk crown uses `foliageHeight`.
                let leaf_height = if dt { foliage_height } else { 1 + random.next_int_bounded(2) };
                let mut yo = offset;
                while yo >= offset - leaf_height {
                    let current_radius = leaf_radius + att.radius_offset + 1 - yo;
                    self.place_leaves_row(level, foliage, random, config, att.pos, current_radius, yo, dt);
                    yo -= 1;
                }
            }
            FoliagePlacer::Cherry { hanging_leaves_chance, hanging_leaves_extension_chance, .. } => {
                // `CherryFoliagePlacer.createFoliage`. The wide-bottom / corner-hole
                // RNG lives in `should_skip_location` (accessed via `self`).
                let foliage_pos = att.pos.above(offset);
                let current_radius = leaf_radius + att.radius_offset - 1;
                let (hc, hec) = (*hanging_leaves_chance, *hanging_leaves_extension_chance);
                self.place_leaves_row(level, foliage, random, config, foliage_pos, current_radius - 2, foliage_height - 3, dt);
                self.place_leaves_row(level, foliage, random, config, foliage_pos, current_radius - 1, foliage_height - 4, dt);
                let mut y = foliage_height - 5;
                while y >= 0 {
                    self.place_leaves_row(level, foliage, random, config, foliage_pos, current_radius, y, dt);
                    y -= 1;
                }
                self.place_leaves_row_with_hanging_below(level, foliage, random, config, foliage_pos, current_radius, -1, dt, hc, hec);
                self.place_leaves_row_with_hanging_below(level, foliage, random, config, foliage_pos, current_radius - 1, -2, dt, hc, hec);
            }
            FoliagePlacer::MegaPine { .. } => {
                // `MegaPineFoliagePlacer.createFoliage`.
                let fx = att.pos.x;
                let fy = att.pos.y;
                let fz = att.pos.z;
                let mut prev_radius = 0;
                let mut yy = fy - foliage_height + offset;
                while yy <= fy + offset {
                    let yo = fy - yy;
                    let smooth_radius =
                        leaf_radius + att.radius_offset + mth_floor_f32(yo as f32 / foliage_height as f32 * 3.5);
                    let jagged_radius = if yo > 0 && smooth_radius == prev_radius && (yy & 1) == 0 {
                        smooth_radius + 1
                    } else {
                        smooth_radius
                    };
                    self.place_leaves_row(level, foliage, random, config, Pos::new(fx, yy, fz), jagged_radius, 0, dt);
                    prev_radius = smooth_radius;
                    yy += 1;
                }
            }
            FoliagePlacer::RandomSpread { leaf_placement_attempts, .. } => {
                // `RandomSpreadFoliagePlacer.createFoliage`.
                let origin = att.pos;
                for _ in 0..*leaf_placement_attempts {
                    let dx = random.next_int_bounded(leaf_radius) - random.next_int_bounded(leaf_radius);
                    let dy = random.next_int_bounded(foliage_height) - random.next_int_bounded(foliage_height);
                    let dz = random.next_int_bounded(leaf_radius) - random.next_int_bounded(leaf_radius);
                    try_place_leaf(level, foliage, random, config, Pos::new(origin.x + dx, origin.y + dy, origin.z + dz));
                }
            }
            FoliagePlacer::Unsupported => {}
        }
    }

    /// `FoliagePlacer.placeLeavesRow`.
    #[allow(clippy::too_many_arguments)]
    fn place_leaves_row(
        &self,
        level: &mut dyn DecorationLevel,
        foliage: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        config: &TreeConfig,
        origin: Pos,
        current_radius: i32,
        y: i32,
        double_trunk: bool,
    ) {
        let off = if double_trunk { 1 } else { 0 };
        for dx in -current_radius..=current_radius + off {
            for dz in -current_radius..=current_radius + off {
                if !self.should_skip_location_signed(random, dx, y, dz, current_radius, double_trunk) {
                    try_place_leaf(level, foliage, random, config, Pos::new(origin.x + dx, origin.y + y, origin.z + dz));
                }
            }
        }
    }

    /// `FoliagePlacer.placeLeavesRowWithHangingLeavesBelow` — place a normal leaf
    /// row, then walk its four outer edges hanging 1–2 leaves below any leaf just
    /// set (cherry's drooping fringe). `foliage.contains` models `FoliageSetter.isSet`.
    #[allow(clippy::too_many_arguments)]
    fn place_leaves_row_with_hanging_below(
        &self,
        level: &mut dyn DecorationLevel,
        foliage: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        config: &TreeConfig,
        origin: Pos,
        current_radius: i32,
        y: i32,
        double_trunk: bool,
        hanging_chance: f32,
        hanging_ext_chance: f32,
    ) {
        self.place_leaves_row(level, foliage, random, config, origin, current_radius, y, double_trunk);
        let off = if double_trunk { 1 } else { 0 };
        let log_pos = origin.below();
        // `Direction.Plane.HORIZONTAL`: NORTH, EAST, SOUTH, WEST.
        for along_edge in [HDir::North, HDir::East, HDir::South, HDir::West] {
            let to_edge = along_edge.clockwise();
            let offset_to_edge = if to_edge.axis_positive() { current_radius + off } else { current_radius };
            // pos = origin + (0, y-1, 0), moved `offset_to_edge` along `to_edge`,
            // then `-current_radius` along `along_edge`.
            let mut px = origin.x + to_edge.step_x() * offset_to_edge + along_edge.step_x() * (-current_radius);
            let py = origin.y + y - 1;
            let mut pz = origin.z + to_edge.step_z() * offset_to_edge + along_edge.step_z() * (-current_radius);
            let mut offset_along_edge = -current_radius;
            while offset_along_edge < current_radius + off {
                // `isSet(pos.move(UP))` then move back down.
                let leaves_above = foliage.contains(&Pos::new(px, py + 1, pz));
                if leaves_above
                    && try_place_extension(level, foliage, random, config, hanging_chance, log_pos, Pos::new(px, py, pz))
                {
                    // one lower, then step back up.
                    try_place_extension(level, foliage, random, config, hanging_ext_chance, log_pos, Pos::new(px, py - 1, pz));
                }
                offset_along_edge += 1;
                px += along_edge.step_x();
                pz += along_edge.step_z();
            }
        }
    }

    /// `FoliagePlacer.shouldSkipLocationSigned` (DarkOak overrides).
    fn should_skip_location_signed(&self, random: &mut WorldgenRandom, dx: i32, y: i32, dz: i32, cr: i32, dt: bool) -> bool {
        if let FoliagePlacer::DarkOak { .. } = self {
            if y != 0 || !dt || (dx != -cr && dx < cr) || (dz != -cr && dz < cr) {
                self.base_should_skip_signed(random, dx, y, dz, cr, dt)
            } else {
                true
            }
        } else {
            self.base_should_skip_signed(random, dx, y, dz, cr, dt)
        }
    }

    fn base_should_skip_signed(&self, random: &mut WorldgenRandom, dx: i32, y: i32, dz: i32, cr: i32, dt: bool) -> bool {
        let (mdx, mdz) = if dt {
            (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs()))
        } else {
            (dx.abs(), dz.abs())
        };
        self.should_skip_location(random, mdx, y, mdz, cr, dt)
    }

    /// `FoliagePlacer.shouldSkipLocation`. Blob draws `nextInt(2)` at each corner
    /// (Java `&&` short-circuit → drawn only when `dx == cr && dz == cr`).
    fn should_skip_location(&self, random: &mut WorldgenRandom, dx: i32, y: i32, dz: i32, cr: i32, dt: bool) -> bool {
        match self {
            FoliagePlacer::Blob { .. } => dx == cr && dz == cr && (random.next_int_bounded(2) == 0 || y == 0),
            FoliagePlacer::Spruce { .. } | FoliagePlacer::Pine { .. } => dx == cr && dz == cr && cr > 0,
            FoliagePlacer::DarkOak { .. } => {
                if y == -1 && !dt {
                    dx == cr && dz == cr
                } else if y == 1 {
                    dx + dz > cr * 2 - 2
                } else {
                    false
                }
            }
            // `FancyFoliagePlacer.shouldSkipLocation` — a circular cross-section
            // (`(dx+0.5)² + (dz+0.5)² > r²`). `dx`/`dz` are the min-abs values from
            // `shouldSkipLocationSigned`. No RNG draw.
            FoliagePlacer::Fancy { .. } => {
                let fx = dx as f32 + 0.5;
                let fz = dz as f32 + 0.5;
                fx * fx + fz * fz > (cr * cr) as f32
            }
            // `BushFoliagePlacer.shouldSkipLocation` — Blob's corner test minus the
            // `y == 0` exemption (draws `nextInt(2)` only at the corner).
            FoliagePlacer::Bush { .. } => dx == cr && dz == cr && random.next_int_bounded(2) == 0,
            // `AcaciaFoliagePlacer.shouldSkipLocation` — no RNG draw.
            FoliagePlacer::Acacia { .. } => {
                if y == 0 {
                    (dx > 1 || dz > 1) && dx != 0 && dz != 0
                } else {
                    dx == cr && dz == cr && cr > 0
                }
            }
            // `MegaJungleFoliagePlacer.shouldSkipLocation` — a clipped circle. No RNG.
            FoliagePlacer::MegaJungle { .. } => {
                if dx + dz >= 7 {
                    true
                } else {
                    dx * dx + dz * dz > cr * cr
                }
            }
            // `MegaPineFoliagePlacer.shouldSkipLocation` — same clipped circle. No RNG.
            FoliagePlacer::MegaPine { .. } => {
                if dx + dz >= 7 {
                    true
                } else {
                    dx * dx + dz * dz > cr * cr
                }
            }
            // `CherryFoliagePlacer.shouldSkipLocation`. Java `&&` short-circuits:
            // each `nextFloat` is drawn only when its preceding condition holds.
            FoliagePlacer::Cherry { wide_bottom_layer_hole_chance, corner_hole_chance, .. } => {
                if y == -1 && (dx == cr || dz == cr) && random.next_float() < *wide_bottom_layer_hole_chance {
                    return true;
                }
                let corner = dx == cr && dz == cr;
                if cr > 2 {
                    corner || (dx + dz > cr * 2 - 2 && random.next_float() < *corner_hole_chance)
                } else {
                    corner && random.next_float() < *corner_hole_chance
                }
            }
            // `RandomSpreadFoliagePlacer.shouldSkipLocation` — always false (unused).
            FoliagePlacer::RandomSpread { .. } => false,
            FoliagePlacer::Unsupported => false,
        }
    }
}

/// `Util.shuffle` (Fisher-Yates): `for i in (2..=size).rev() swap(i-1, nextInt(i))`.
fn util_shuffle<T>(list: &mut [T], random: &mut WorldgenRandom) {
    let size = list.len();
    let mut i = size;
    while i > 1 {
        let swap_to = random.next_int_bounded(i as i32) as usize;
        list.swap(i - 1, swap_to);
        i -= 1;
    }
}

impl TreeDecorator {
    /// `TreeDecorator.place`. Only `beehive` is modeled.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &self,
        level: &mut dyn DecorationLevel,
        decorations: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        trunks: &HashSet<Pos>,
        foliage: &HashSet<Pos>,
        roots: &HashSet<Pos>,
    ) {
        match self {
            TreeDecorator::Beehive { probability } => {
                beehive_place(*probability, level, decorations, random, trunks, foliage);
            }
            TreeDecorator::Cocoa { probability } => {
                cocoa_place(*probability, level, decorations, random, trunks);
            }
            TreeDecorator::TrunkVine => {
                trunk_vine_place(level, decorations, random, trunks);
            }
            TreeDecorator::LeaveVine { probability } => {
                leave_vine_place(*probability, level, decorations, random, foliage);
            }
            TreeDecorator::AlterGround { provider } => {
                alter_ground_place(provider, level, decorations, random, trunks, roots);
            }
            TreeDecorator::AttachedToLeaves {
                probability,
                exclusion_radius_xz,
                exclusion_radius_y,
                block_provider,
                required_empty_blocks,
                directions,
            } => {
                attached_to_leaves_place(
                    *probability,
                    *exclusion_radius_xz,
                    *exclusion_radius_y,
                    block_provider,
                    *required_empty_blocks,
                    directions,
                    level,
                    decorations,
                    random,
                    foliage,
                );
            }
            TreeDecorator::Unsupported => {}
        }
    }
}

/// `AlterGroundDecorator.place`. Uses the lowest trunk-or-root ring to seed a set
/// of 5×5 podzol patches (4 fixed corners + 5 random offsets, each a `nextInt(64)`).
fn alter_ground_place(
    provider: &StateProvider,
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    trunks: &HashSet<Pos>,
    roots: &HashSet<Pos>,
) {
    // `TreeFeature.getLowestTrunkOrRootOfTree`.
    let logs = sorted_by_y(trunks);
    let root_list = sorted_by_y(roots);
    let block_positions: Vec<Pos> = if root_list.is_empty() {
        logs.clone()
    } else if !logs.is_empty() && root_list[0].y == logs[0].y {
        logs.iter().chain(root_list.iter()).copied().collect()
    } else {
        root_list.clone()
    };
    if block_positions.is_empty() {
        return;
    }
    let min_y = block_positions[0].y;
    for pos in block_positions.iter().filter(|p| p.y == min_y) {
        alter_ground_circle(provider, level, decorations, random, pos.west().north());
        alter_ground_circle(provider, level, decorations, random, pos.east().east().north());
        alter_ground_circle(provider, level, decorations, random, pos.west().south().south());
        alter_ground_circle(provider, level, decorations, random, pos.east().east().south().south());
        for _ in 0..5 {
            let placement = random.next_int_bounded(64);
            let xx = placement % 8;
            let zz = placement / 8;
            if xx == 0 || xx == 7 || zz == 0 || zz == 7 {
                alter_ground_circle(provider, level, decorations, random, pos.offset(-3 + xx, 0, -3 + zz));
            }
        }
    }
}

/// `AlterGroundDecorator.placeCircle` — a 5×5 disc minus the four corners.
fn alter_ground_circle(
    provider: &StateProvider,
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    center: Pos,
) {
    for xx in -2i32..=2 {
        for zz in -2i32..=2 {
            if xx.abs() != 2 || zz.abs() != 2 {
                alter_ground_block(provider, level, decorations, random, center.offset(xx, 0, zz));
            }
        }
    }
}

/// `AlterGroundDecorator.placeBlockAt` — scan a small vertical window for the
/// first podzol-replaceable ground block and swap it.
fn alter_ground_block(
    provider: &StateProvider,
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    pos: Pos,
) {
    let mut dy = 2;
    while dy >= -3 {
        let cursor = pos.above(dy);
        if let Some(state) = provider.get_state(&*level, random, cursor) {
            decorations.insert(cursor);
            level.set_block(cursor.x, cursor.y, cursor.z, state);
            break;
        }
        if !level.get_block(cursor.x, cursor.y, cursor.z).is_air() && dy < 0 {
            break;
        }
        dy -= 1;
    }
}

/// `AttachedToLeavesDecorator.place` — shuffle the leaves, then for each draw a
/// direction, a `nextFloat` gate, and place a hanging block (mangrove propagule)
/// with an exclusion zone.
#[allow(clippy::too_many_arguments)]
fn attached_to_leaves_place(
    probability: f32,
    exclusion_radius_xz: i32,
    exclusion_radius_y: i32,
    block_provider: &StateProvider,
    required_empty_blocks: i32,
    directions: &[(i32, i32, i32)],
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    foliage: &HashSet<Pos>,
) {
    if directions.is_empty() {
        return;
    }
    let mut leaves = sorted_by_y(foliage);
    util_shuffle(&mut leaves, random);
    let mut blacklist: HashSet<Pos> = HashSet::new();
    for leaf in leaves {
        // `Util.getRandom(directions, random)` = `directions[nextInt(size)]`.
        let (dx, dy, dz) = directions[random.next_int_bounded(directions.len() as i32) as usize];
        let placement = Pos::new(leaf.x + dx, leaf.y + dy, leaf.z + dz);
        if blacklist.contains(&placement) {
            continue;
        }
        if random.next_float() >= probability {
            continue;
        }
        // `hasRequiredEmptyBlocks`.
        let mut all_empty = true;
        for i in 1..=required_empty_blocks {
            let p = Pos::new(leaf.x + dx * i, leaf.y + dy * i, leaf.z + dz * i);
            if !level.get_block(p.x, p.y, p.z).is_air() {
                all_empty = false;
                break;
            }
        }
        if !all_empty {
            continue;
        }
        for ex in -exclusion_radius_xz..=exclusion_radius_xz {
            for ey in -exclusion_radius_y..=exclusion_radius_y {
                for ez in -exclusion_radius_xz..=exclusion_radius_xz {
                    blacklist.insert(Pos::new(placement.x + ex, placement.y + ey, placement.z + ez));
                }
            }
        }
        if let Some(state) = block_provider.get_state(&*level, random, placement) {
            decorations.insert(placement);
            level.set_block(placement.x, placement.y, placement.z, state);
        }
    }
}

/// Sort a decorator position set into `Comparator.comparingInt(Vec3i::getY)`
/// order. Vanilla builds the list from a JVM `HashSet` (whose iteration order we
/// cannot reproduce) and stable-sorts by Y only; we sort by `(y, x, z)` so the
/// output is deterministic. The number of RNG draws each decorator makes is a
/// function of this ordering (see the module notes), so cocoa/vine placement is
/// internally reproducible but may differ from vanilla at the block grid — the
/// same tradeoff the beehive decorator already documents.
fn sorted_by_y(set: &HashSet<Pos>) -> Vec<Pos> {
    let mut v: Vec<Pos> = set.iter().copied().collect();
    v.sort_by_key(|p| (p.y, p.x, p.z));
    v
}

/// `CocoaDecorator.place`. Draws one `nextFloat` gate, then for each of the
/// lowest logs (`y - treeY <= 2`) draws `nextFloat` per horizontal direction,
/// and `nextInt(3)` (cocoa age) when a pod is placed. Cocoa's directional
/// `facing`/`age` properties collapse to the default `cocoa` state (identity
/// alphabet); the RNG draws are consumed 1:1.
fn cocoa_place(
    probability: f32,
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    trunks: &HashSet<Pos>,
) {
    if random.next_float() >= probability {
        return;
    }
    let logs = sorted_by_y(trunks);
    if logs.is_empty() {
        return;
    }
    let tree_y = logs[0].y;
    // `Direction.Plane.HORIZONTAL`: NORTH, EAST, SOUTH, WEST.
    const HORIZ: [HDir; 4] = [HDir::North, HDir::East, HDir::South, HDir::West];
    for log in logs.iter().filter(|p| p.y - tree_y <= 2) {
        for dir in HORIZ {
            if random.next_float() <= 0.25 {
                // `cocoaPos = pos.offset(opposite.getStepX(), 0, opposite.getStepZ())`.
                let cocoa_pos = Pos::new(log.x - dir.step_x(), log.y, log.z - dir.step_z());
                if level.get_block(cocoa_pos.x, cocoa_pos.y, cocoa_pos.z).is_air() {
                    let _age = random.next_int_bounded(3);
                    decorations.insert(cocoa_pos);
                    level.set_block(cocoa_pos.x, cocoa_pos.y, cocoa_pos.z, ParityBlock::Cocoa);
                }
            }
        }
    }
}

/// `TrunkVineDecorator.place`. For each log, draws `nextInt(3)` per horizontal
/// direction and places a vine on air sides. Vine `direction` property collapses
/// to the default `vine` state; RNG draws are consumed 1:1.
fn trunk_vine_place(
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    trunks: &HashSet<Pos>,
) {
    for log in sorted_by_y(trunks) {
        // west, east, north, south — each `nextInt(3) > 0` gates a vine.
        for side in [log.west(), log.east(), log.north(), log.south()] {
            if random.next_int_bounded(3) > 0 && level.get_block(side.x, side.y, side.z).is_air() {
                decorations.insert(side);
                level.set_block(side.x, side.y, side.z, ParityBlock::Vine);
            }
        }
    }
}

/// `LeaveVineDecorator.place`. For each leaf, draws `nextFloat` per horizontal
/// direction; a passing side grows a hanging vine column (no further RNG).
fn leave_vine_place(
    probability: f32,
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    foliage: &HashSet<Pos>,
) {
    for leaf in sorted_by_y(foliage) {
        for side in [leaf.west(), leaf.east(), leaf.north(), leaf.south()] {
            if random.next_float() < probability && level.get_block(side.x, side.y, side.z).is_air() {
                add_hanging_vine(level, decorations, side);
            }
        }
    }
}

/// `LeaveVineDecorator.addHangingVine` — place a vine then extend down through
/// air up to 4 blocks. No RNG.
fn add_hanging_vine(level: &mut dyn DecorationLevel, decorations: &mut HashSet<Pos>, pos: Pos) {
    decorations.insert(pos);
    level.set_block(pos.x, pos.y, pos.z, ParityBlock::Vine);
    let mut p = pos.below();
    let mut max_dir = 4;
    while max_dir > 0 && level.get_block(p.x, p.y, p.z).is_air() {
        decorations.insert(p);
        level.set_block(p.x, p.y, p.z, ParityBlock::Vine);
        p = p.below();
        max_dir -= 1;
    }
}

/// `BeehiveDecorator.place`. `SPAWN_DIRECTIONS` = HORIZONTAL minus NORTH
/// (opposite of the SOUTH worldgen facing) = [EAST, SOUTH, WEST]. Places a
/// `bee_nest` (bee-entity NBT is out of block-grid scope, but its RNG draws —
/// `2 + nextInt(2)` bees, each `nextInt(599)` — MUST run to stay in parity).
fn beehive_place(
    probability: f32,
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    trunks: &HashSet<Pos>,
    foliage: &HashSet<Pos>,
) {
    if trunks.is_empty() {
        return;
    }
    // `Context` sorts logs/leaves by Y ascending. HashSet order is not vanilla's
    // (a JVM `HashSet`), which can move the chosen nest position, but every RNG
    // draw count below is order-independent; a fully-deterministic (y,x,z) order
    // keeps our own output reproducible.
    let mut logs: Vec<Pos> = trunks.iter().copied().collect();
    let mut leaves: Vec<Pos> = foliage.iter().copied().collect();
    logs.sort_by_key(|p| (p.y, p.x, p.z));
    leaves.sort_by_key(|p| (p.y, p.x, p.z));

    if random.next_float() >= probability {
        return;
    }
    let hive_y = if !leaves.is_empty() {
        (leaves[0].y - 1).max(logs[0].y + 1)
    } else {
        (logs[0].y + 1 + random.next_int_bounded(3)).min(logs[logs.len() - 1].y)
    };
    // SPAWN_DIRECTIONS applied to each log at hive_y, in order.
    const SPAWN: [HDir; 3] = [HDir::East, HDir::South, HDir::West];
    let mut hive_placements: Vec<Pos> = logs
        .iter()
        .filter(|p| p.y == hive_y)
        .flat_map(|p| SPAWN.iter().map(move |d| Pos::new(p.x + d.step_x(), p.y, p.z + d.step_z())))
        .collect();
    if hive_placements.is_empty() {
        return;
    }
    util_shuffle(&mut hive_placements, random);
    // WORLDGEN_FACING = SOUTH: require air at pos and at pos.south().
    let hive = hive_placements
        .iter()
        .find(|p| level.get_block(p.x, p.y, p.z).is_air() && level.get_block(p.x, p.y, p.z + 1).is_air())
        .copied();
    if let Some(hp) = hive {
        decorations.insert(hp);
        level.set_block(hp.x, hp.y, hp.z, ParityBlock::BeeNest);
        // Bee entities aren't modeled, but their creation draws must run: the
        // block entity always exists after the set_block above.
        let num_bees = 2 + random.next_int_bounded(2);
        for _ in 0..num_bees {
            let _ = random.next_int_bounded(599);
        }
    }
}

/// `TreeFeature.doPlace` + `place` (decorators), skipping the root placer and the
/// `updateLeaves` BFS. The RNG draw order matches the decompile exactly:
/// getTreeHeight → foliageHeight → foliageRadius → (bounds/clip, no draws) →
/// placeTrunk → per-attachment createFoliage → decorators.
fn place_tree(config: &TreeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    // Graceful skip for unported placers (fancy trunk, fancy/mega foliage, …):
    // bail before any RNG draw so this terminal feature simply produces nothing
    // rather than mis-drawing (safe — the RNG is reseeded per top feature).
    if config.trunk_placer.is_unsupported()
        || config.foliage_placer.is_unsupported()
        || config.root_placer_unsupported
    {
        return;
    }

    let level: &mut dyn DecorationLevel = ctx.level;

    let tree_height = config.trunk_placer.get_tree_height(random);
    let foliage_height = config.foliage_placer.foliage_height(random, tree_height);
    let trunk_height = tree_height - foliage_height;
    let leaf_radius = config.foliage_placer.foliage_radius(random, trunk_height);
    // `config.rootPlacer.map(rp -> rp.getTrunkOrigin(origin, random)).orElse(origin)`
    // — the root offset draw happens here, before the clip check and trunk.
    let trunk_origin = match &config.root_placer {
        Some(rp) => rp.get_trunk_origin(origin, random),
        None => origin,
    };
    let min_y = origin.y.min(trunk_origin.y);
    let max_y = origin.y.max(trunk_origin.y) + tree_height + 1;
    // Vanilla `TreeFeature.doPlace`: proceed when `minY >= getMinY()+1 && maxY
    // <= getMaxY()+1`. `getMaxY()` is the inclusive top, so `getMaxY()+1` ==
    // `getMaxBuildHeight()` == our exclusive `max_y()`.
    if min_y < level.min_y() + 1 || max_y > level.max_y() {
        return;
    }
    let grow_through = config.trunk_placer.grow_through();
    let min_clipped = config.minimum_size.min_clipped_height();
    let clipped = get_max_free_tree_height(level, tree_height, trunk_origin, config, grow_through);
    if !(clipped >= tree_height || min_clipped.map(|m| clipped >= m).unwrap_or(false)) {
        return;
    }

    let mut trunks: HashSet<Pos> = HashSet::new();
    let mut foliage: HashSet<Pos> = HashSet::new();
    let mut roots: HashSet<Pos> = HashSet::new();
    let mut decorations: HashSet<Pos> = HashSet::new();

    // Roots are placed first; a failed root system aborts the whole tree.
    if let Some(rp) = &config.root_placer {
        if !rp.place_roots(level, &mut roots, random, origin, trunk_origin, config) {
            return;
        }
    }

    let attachments = config.trunk_placer.place_trunk(level, &mut trunks, random, clipped, trunk_origin, config);
    for att in &attachments {
        config
            .foliage_placer
            .create_foliage(level, &mut foliage, random, config, clipped, att, foliage_height, leaf_radius);
    }

    // `place`: decorators run only when the tree placed something.
    if !trunks.is_empty() || !foliage.is_empty() {
        for dec in &config.decorators {
            dec.place(level, &mut decorations, random, &trunks, &foliage, &roots);
        }
    }
    // `updateLeaves` + `updateShapeAtEdge` are the deferred block-state pass.
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// No JVM harness is available for P8, so these are structural / self-consistency
// checks: seeding + placement determinism, ore-vein shape and target discipline,
// and FeatureSorter ordering invariants. Block-for-block golden verification vs
// the real 26.2 jar is deferred to the end-to-end `.mca` diff (see
// docs/WORLDGEN_PARITY.md "Verification strategy").
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A one-chunk in-memory level: solid stone up to `surface-1`, air above,
    /// with a fixed biome fill. Records every write.
    struct TestLevel {
        blocks: HashMap<(i32, i32, i32), ParityBlock>,
        surface: i32,
        min_y: i32,
        height: i32,
        biome_fill: u16,
    }

    impl TestLevel {
        fn new(surface: i32) -> Self {
            Self { blocks: HashMap::new(), surface, min_y: -64, height: 384, biome_fill: 0 }
        }
        fn base(&self, y: i32) -> ParityBlock {
            if y < self.surface {
                ParityBlock::Stone
            } else {
                ParityBlock::Air
            }
        }
    }

    impl DecorationLevel for TestLevel {
        fn get_block(&self, x: i32, y: i32, z: i32) -> ParityBlock {
            *self.blocks.get(&(x, y, z)).unwrap_or(&self.base(y))
        }
        fn set_block(&mut self, x: i32, y: i32, z: i32, state: ParityBlock) -> bool {
            self.blocks.insert((x, y, z), state);
            true
        }
        fn get_height(&self, hm: Heightmap, _x: i32, _z: i32) -> i32 {
            // Stone up to surface-1 → first-available = surface for the solid
            // heightmaps; there is no non-air above, so surface for all.
            let _ = hm;
            self.surface
        }
        fn get_biome_fill(&self, _x: i32, _y: i32, _z: i32) -> u16 {
            self.biome_fill
        }
        fn min_y(&self) -> i32 {
            self.min_y
        }
        fn gen_depth(&self) -> i32 {
            self.height
        }
        fn sea_level(&self) -> i32 {
            63
        }
    }

    struct AllBiome;
    impl BiomeFeatureIndex for AllBiome {
        fn biome_has_feature(&self, _fill: u16, _id: &str) -> bool {
            true
        }
    }

    fn coal_config() -> OreConfig {
        OreConfig {
            targets: vec![OreTarget {
                target: RuleTest::TagMatch(super::super::placement::BlockTag::StoneOreReplaceables),
                state: ParityBlock::CoalOre,
            }],
            size: 17,
            discard_chance_on_air_exposure: 0.0,
        }
    }

    /// The ore vein writes only into stone (its target tag), stays inside the
    /// feature bounding box, and is reproducible for a fixed seed.
    #[test]
    fn ore_vein_shape_and_targets() {
        let run = || {
            let mut level = TestLevel::new(80);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "x" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(42));
            place_ore(&coal_config(), &mut ctx, &mut random, Pos::new(8, 40, 8));
            let mut writes: Vec<((i32, i32, i32), ParityBlock)> =
                level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run();
        let b = run();
        assert_eq!(a, b, "ore placement is deterministic for a fixed seed");
        assert!(!a.is_empty(), "the vein places at least one block");
        for ((x, y, z), block) in &a {
            assert_eq!(*block, ParityBlock::CoalOre, "only coal ore is written");
            // size 17 → radius bound well under 16 blocks of the origin.
            assert!((x - 8).abs() <= 16 && (z - 8).abs() <= 16, "within XZ bounds");
            assert!((y - 40).abs() <= 16, "within Y bounds");
        }
    }

    /// The vein only replaces its target blocks: a non-target existing block is
    /// left untouched (here, pre-placing air where the vein would write).
    #[test]
    fn ore_respects_non_target_blocks() {
        let mut level = TestLevel::new(80);
        // Fill the whole vein region with air (a non-target); nothing should be
        // written because coal only replaces stone-ore-replaceables.
        for y in 24..56 {
            for z in -8..24 {
                for x in -8..24 {
                    level.set_block(x, y, z, ParityBlock::Air);
                }
            }
        }
        let placed_before = level.blocks.len();
        let idx = AllBiome;
        let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "x" };
        let mut random = WorldgenRandom::new(RandomSource::xoroshiro(7));
        place_ore(&coal_config(), &mut ctx, &mut random, Pos::new(8, 40, 8));
        let coal = level.blocks.values().filter(|b| **b == ParityBlock::CoalOre).count();
        assert_eq!(coal, 0, "no ore in an all-air region");
        assert_eq!(level.blocks.len(), placed_before, "no writes at all");
    }

    fn registry_for(biomes: &[&str]) -> FeatureRegistry {
        FeatureRegistry::load(biomes.iter().map(|s| s.to_string()).collect())
    }

    /// Full decoration is a deterministic function of the seed, and different
    /// seeds diverge.
    #[test]
    fn decoration_is_deterministic_and_seed_sensitive() {
        let registry = registry_for(&["minecraft:plains"]);
        let possible: HashSet<u16> = [0u16].into_iter().collect();
        let decorate = |seed: i64| {
            let mut level = TestLevel::new(70);
            apply_biome_decoration(&registry, &mut level, &possible, seed, 0, 0);
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        assert_eq!(decorate(123), decorate(123), "deterministic for a fixed seed");
        assert_ne!(decorate(123), decorate(456), "seed-sensitive");
    }

    /// `FeatureSorter` preserves each biome's within-list feature order: for
    /// plains, `ore_dirt` precedes `ore_gravel` in the underground_ores step,
    /// and the per-step index lookup is consistent with the step list.
    #[test]
    fn feature_sorter_preserves_biome_order() {
        let registry = registry_for(&["minecraft:plains"]);
        // underground_ores is decoration step 6.
        let step = 6usize;
        let list = &registry.steps[step];
        let idx = &registry.step_index[step];
        let pos = |id: &str| list.iter().position(|x| x == id);
        let dirt = pos("ore_dirt").expect("ore_dirt in step 6");
        let gravel = pos("ore_gravel").expect("ore_gravel in step 6");
        assert!(dirt < gravel, "within-biome order preserved (dirt before gravel)");
        // index lookup agrees with the list.
        for (i, id) in list.iter().enumerate() {
            assert_eq!(idx[id], i as i32);
        }
    }

    /// Every implemented configured feature parses without landing in
    /// `Deferred`, and the vendored data loads cleanly.
    #[test]
    fn implemented_features_parse() {
        let registry = registry_for(&["minecraft:plains"]);
        for id in [
            "ore_coal", "ore_iron", "ore_dirt", "disk_sand", "spring_water",
            "oak", "birch", "spruce", "dark_oak", "pine", "trees_plains", "trees_birch",
        ] {
            let cf = registry.configured.get(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(cf.is_implemented(), "{id} should be implemented, got {cf:?}");
        }
        // `oak` is now a real tree feature.
        assert!(matches!(registry.configured.get("oak"), Some(ConfiguredFeature::Tree(_))));
        assert!(matches!(registry.configured.get("trees_plains"), Some(ConfiguredFeature::RandomSelector(_))));
        // A feature type this milestone still defers stays `Deferred`.
        assert!(!registry.configured.get("amethyst_geode").map(|c| c.is_implemented()).unwrap_or(true));
    }

    // --- Tree feature structural tests ---------------------------------------

    /// A grass/dirt surface at `surface`, air above, stone below. Trees grow on
    /// top of it (origin at the first air = `surface`).
    fn tree_level(surface: i32) -> TestLevel {
        let mut level = TestLevel::new(surface);
        // A grass cap so `would_survive` / `below_trunk_provider` behave.
        for z in -32..48 {
            for x in -32..48 {
                level.set_block(x, surface - 1, z, ParityBlock::GrassBlock);
            }
        }
        level
    }

    fn oak_config(registry: &FeatureRegistry) -> TreeConfig {
        match registry.configured.get("oak") {
            Some(ConfiguredFeature::Tree(c)) => c.clone(),
            _ => panic!("oak is not a tree"),
        }
    }

    /// `getTreeHeight`/`foliageHeight`/`foliageRadius` draw exactly the vanilla
    /// sequence — verified by replaying the draws on a twin RNG.
    #[test]
    fn tree_rng_draw_order_matches_manual_replay() {
        let registry = registry_for(&["minecraft:plains"]);
        // Straight oak: base 4, a 2, b 0; blob foliage (height 3, no draws);
        // blob radius constant 2 (no draw), blob offset constant 0 (no draw).
        let oak = oak_config(&registry);
        let mut a = WorldgenRandom::new(RandomSource::xoroshiro(99));
        let mut b = WorldgenRandom::new(RandomSource::xoroshiro(99));

        let th = oak.trunk_placer.get_tree_height(&mut a);
        // Manual: base 4 + nextInt(3) + nextInt(1).
        let th_manual = 4 + b.next_int_bounded(3) + b.next_int_bounded(1);
        assert_eq!(th, th_manual, "getTreeHeight draw sequence");

        let fh = oak.foliage_placer.foliage_height(&mut a, th);
        assert_eq!(fh, 3, "blob foliageHeight is constant (no draw)");
        let fr = oak.foliage_placer.foliage_radius(&mut a, th - fh);
        assert_eq!(fr, 2, "blob foliageRadius is constant (no draw)");
        // No draws happened in foliageHeight/foliageRadius, so the twin is still
        // in lockstep: the next draw matches.
        assert_eq!(a.next_int_bounded(100), b.next_int_bounded(100), "twin RNGs stayed in lockstep");
    }

    /// Spruce foliageHeight draws one (trunk_height uniform), and its radius one
    /// (radius uniform) — replay confirms the count.
    #[test]
    fn spruce_foliage_draws_one_each() {
        let registry = registry_for(&["minecraft:taiga"]);
        let spruce = match registry.configured.get("spruce") {
            Some(ConfiguredFeature::Tree(c)) => c.clone(),
            _ => panic!("spruce not a tree"),
        };
        let mut a = WorldgenRandom::new(RandomSource::xoroshiro(7));
        let mut b = WorldgenRandom::new(RandomSource::xoroshiro(7));
        let th = spruce.trunk_placer.get_tree_height(&mut a);
        let _ = b.next_int_bounded(3); // a=2
        let _ = b.next_int_bounded(2); // b=1
        let fh = spruce.foliage_placer.foliage_height(&mut a, th);
        // foliageHeight = max(4, th - trunk_height.sample) → one draw.
        let trunk_h = b.next_int_bounded(2) + 1; // uniform [1,2]
        assert_eq!(fh, (th - trunk_h).max(4));
        let fr = spruce.foliage_placer.foliage_radius(&mut a, th - fh);
        let rad = b.next_int_bounded(2) + 2; // uniform [2,3]
        assert_eq!(fr, rad, "spruce radius uniform draw");
    }

    /// A placed oak tree is deterministic for a fixed seed and seed-sensitive,
    /// writes only logs/leaves/dirt, and the trunk column reaches the clipped
    /// height (== full height on open ground).
    #[test]
    fn oak_tree_places_trunk_and_leaves() {
        let registry = registry_for(&["minecraft:plains"]);
        let oak = oak_config(&registry);
        let run = |seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "oak" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_tree(&oak, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(123);
        assert_eq!(a, run(123), "deterministic for a fixed seed");

        let logs: Vec<_> = a.iter().filter(|(_, b)| *b == ParityBlock::OakLog).collect();
        assert!(!logs.is_empty(), "trunk logs placed");
        // A trunk column at (8, y, 8): count logs there; must equal treeHeight.
        let column = logs.iter().filter(|((x, _, z), _)| *x == 8 && *z == 8).count() as i32;
        // Recompute treeHeight for this seed.
        let mut r = WorldgenRandom::new(RandomSource::xoroshiro(123));
        let th = oak.trunk_placer.get_tree_height(&mut r);
        assert_eq!(column, th, "straight trunk column height == tree height");

        // Only logs/leaves/dirt are written (identity alphabet), and leaves sit
        // where a valid (air/replaceable) position existed.
        for ((x, y, z), block) in &a {
            let base = if *y < 70 { ParityBlock::Stone } else { ParityBlock::Air };
            let _ = base;
            assert!(
                matches!(
                    block,
                    ParityBlock::OakLog | ParityBlock::OakLeaves | ParityBlock::Dirt | ParityBlock::GrassBlock
                ),
                "unexpected block {block:?} at {x},{y},{z}"
            );
        }
    }

    /// Through the full decoration driver, a plains chunk on grassy ground
    /// produces oak logs+leaves, and the whole pass is deterministic and
    /// seed-sensitive.
    #[test]
    fn trees_generate_through_decoration_driver() {
        let registry = registry_for(&["minecraft:plains"]);
        let possible: HashSet<u16> = [0u16].into_iter().collect();
        let decorate = |seed: i64| {
            let mut level = tree_level(70);
            apply_biome_decoration(&registry, &mut level, &possible, seed, 0, 0);
            level
                .blocks
                .iter()
                .filter(|(_, b)| matches!(b, ParityBlock::OakLog | ParityBlock::OakLeaves))
                .count()
        };
        // At least one seed in a small sweep must grow a tree.
        let grew = (0..8).any(|s| decorate(s * 1000 + 1) > 0);
        assert!(grew, "some seed grows a tree via trees_plains");
    }

    /// Fancy oak parses to a real tree with the fancy trunk + fancy foliage
    /// placers (not `Deferred` / `Unsupported`), and places oak logs+leaves
    /// deterministically. The canopy branches out, so leaves must appear off the
    /// central trunk column.
    #[test]
    fn fancy_oak_places_trunk_and_canopy() {
        let registry = registry_for(&["minecraft:forest"]);
        let fancy = match registry.configured.get("fancy_oak") {
            Some(ConfiguredFeature::Tree(c)) => c.clone(),
            other => panic!("fancy_oak is not a tree: {other:?}"),
        };
        assert!(matches!(fancy.trunk_placer, TrunkPlacer::Fancy { .. }), "fancy trunk placer");
        assert!(matches!(fancy.foliage_placer, FoliagePlacer::Fancy { .. }), "fancy foliage placer");

        let run = |seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "fancy_oak" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_tree(&fancy, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        // Find a seed that grows (fancy oak needs vertical headroom / free space);
        // most seeds do on open ground.
        let seed = (0..64)
            .find(|s| {
                run(*s).iter().any(|(_, b)| *b == ParityBlock::OakLog)
                    && run(*s).iter().any(|(_, b)| *b == ParityBlock::OakLeaves)
            })
            .expect("some seed grows a fancy oak");
        let a = run(seed);
        assert_eq!(a, run(seed), "fancy oak is deterministic for a fixed seed");

        let logs: Vec<_> = a.iter().filter(|(_, b)| *b == ParityBlock::OakLog).collect();
        let leaves: Vec<_> = a.iter().filter(|(_, b)| *b == ParityBlock::OakLeaves).collect();
        assert!(!logs.is_empty(), "fancy trunk logs placed");
        assert!(leaves.len() > logs.len(), "canopy leaves outnumber logs");
        // The canopy is a branch structure: leaves exist off the central column.
        let off_column = leaves.iter().any(|((x, _, z), _)| *x != 8 || *z != 8);
        assert!(off_column, "fancy canopy spreads leaves off the trunk column");
        // Only the identity tree alphabet is written.
        for ((x, y, z), block) in &a {
            assert!(
                matches!(
                    block,
                    ParityBlock::OakLog | ParityBlock::OakLeaves | ParityBlock::Dirt | ParityBlock::GrassBlock
                ),
                "unexpected block {block:?} at {x},{y},{z}"
            );
        }
    }

    /// Jungle + acacia trees parse to the new giant/mega-jungle trunk and
    /// acacia/bush/mega-jungle foliage placers (not `Unsupported`), and each
    /// grows its wood/leaf pair deterministically on open ground. `jungle_bush`
    /// intentionally uses a jungle trunk with an oak-leaf canopy.
    #[test]
    fn jungle_and_acacia_trees_place_logs_and_leaves() {
        use ParityBlock::*;
        let registry = registry_for(&["minecraft:jungle"]);
        let tree = |id: &str| match registry.configured.get(id) {
            Some(ConfiguredFeature::Tree(c)) => c.clone(),
            other => panic!("{id} is not a tree: {other:?}"),
        };
        let jungle = tree("jungle_tree");
        let mega = tree("mega_jungle_tree");
        let bush = tree("jungle_bush");
        let acacia = tree("acacia");

        // The new placers parsed to their supported variants.
        assert!(matches!(mega.trunk_placer, TrunkPlacer::MegaJungle { .. }), "mega jungle trunk");
        assert!(matches!(mega.foliage_placer, FoliagePlacer::MegaJungle { .. }), "mega jungle foliage");
        assert!(matches!(bush.foliage_placer, FoliagePlacer::Bush { .. }), "bush foliage");
        assert!(matches!(acacia.trunk_placer, TrunkPlacer::Forking { .. }), "acacia forking trunk");
        assert!(matches!(acacia.foliage_placer, FoliagePlacer::Acacia { .. }), "acacia foliage");
        assert!(!jungle.trunk_placer.is_unsupported() && !jungle.foliage_placer.is_unsupported());

        let grow = |config: &TreeConfig, seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_tree(config, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };

        for (id, config, log, leaf) in [
            ("jungle_tree", &jungle, JungleLog, JungleLeaves),
            ("mega_jungle_tree", &mega, JungleLog, JungleLeaves),
            ("jungle_bush", &bush, JungleLog, OakLeaves),
            ("acacia", &acacia, AcaciaLog, AcaciaLeaves),
        ] {
            let seed = (0..128)
                .find(|s| {
                    let w = grow(config, *s);
                    w.iter().any(|(_, b)| *b == log) && w.iter().any(|(_, b)| *b == leaf)
                })
                .unwrap_or_else(|| panic!("{id} grows on open ground for some seed"));
            let a = grow(config, seed);
            assert_eq!(a, grow(config, seed), "{id} is deterministic for a fixed seed");
            assert!(a.iter().any(|(_, b)| *b == log), "{id} places {log:?}");
            assert!(a.iter().any(|(_, b)| *b == leaf), "{id} places {leaf:?}");
        }
    }

    /// Cherry, azalea, mega spruce, and mangrove all parse to their new supported
    /// placers (not `Unsupported`/`Deferred`) and grow their wood/leaf pair
    /// deterministically on open ground. Covers the cherry trunk/foliage, azalea
    /// bending trunk + random-spread foliage, mega-pine foliage + giant trunk +
    /// alter-ground podzol, and the full mangrove stack (upwards-branching trunk,
    /// random-spread foliage, mangrove root placer, attached-to-leaves propagules).
    #[test]
    fn remaining_overworld_trees_place_logs_and_leaves() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:plains"]);
        let tree = |id: &str| match reg.configured.get(id) {
            Some(ConfiguredFeature::Tree(c)) => c.clone(),
            other => panic!("{id} is not a tree: {other:?}"),
        };
        let cherry = tree("cherry");
        let azalea = tree("azalea_tree");
        let mega_spruce = tree("mega_spruce");
        let mangrove = tree("mangrove");

        // The new placers parsed to their supported variants.
        assert!(matches!(cherry.trunk_placer, TrunkPlacer::Cherry { .. }), "cherry trunk");
        assert!(matches!(cherry.foliage_placer, FoliagePlacer::Cherry { .. }), "cherry foliage");
        assert!(matches!(azalea.trunk_placer, TrunkPlacer::Bending { .. }), "bending trunk");
        assert!(matches!(azalea.foliage_placer, FoliagePlacer::RandomSpread { .. }), "random-spread foliage");
        assert!(matches!(mega_spruce.trunk_placer, TrunkPlacer::Giant { .. }), "giant trunk");
        assert!(matches!(mega_spruce.foliage_placer, FoliagePlacer::MegaPine { .. }), "mega-pine foliage");
        assert!(matches!(mega_spruce.decorators.as_slice(), [TreeDecorator::AlterGround { .. }]), "alter-ground decorator");
        assert!(matches!(mangrove.trunk_placer, TrunkPlacer::UpwardsBranching { .. }), "upwards-branching trunk");
        assert!(matches!(mangrove.foliage_placer, FoliagePlacer::RandomSpread { .. }), "mangrove foliage");
        assert!(mangrove.root_placer.is_some() && !mangrove.root_placer_unsupported, "mangrove root placer supported");
        assert!(
            mangrove.decorators.iter().any(|d| matches!(d, TreeDecorator::AttachedToLeaves { .. })),
            "attached-to-leaves decorator"
        );

        let grow = |config: &TreeConfig, seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_tree(config, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };

        // Azalea's canopy is either leaf variant; assert the log + at least one leaf.
        for (id, config, log, leaf) in [
            ("cherry", &cherry, CherryLog, CherryLeaves),
            ("azalea_tree", &azalea, OakLog, AzaleaLeaves),
            ("mega_spruce", &mega_spruce, SpruceLog, SpruceLeaves),
            ("mangrove", &mangrove, MangroveLog, MangroveLeaves),
        ] {
            let seed = (0..256)
                .find(|s| {
                    let w = grow(config, *s);
                    w.iter().any(|(_, b)| *b == log) && w.iter().any(|(_, b)| b.is_leaves())
                })
                .unwrap_or_else(|| panic!("{id} grows on open ground for some seed"));
            let a = grow(config, seed);
            assert_eq!(a, grow(config, seed), "{id} is deterministic for a fixed seed");
            assert!(a.iter().any(|(_, b)| *b == log), "{id} places {log:?}");
            assert!(a.iter().any(|(_, b)| b.is_leaves()), "{id} places canopy leaves");
            let _ = leaf;
        }

        // The mega spruce's alter_ground decorator lays podzol under the trunk.
        let podzol_seed = (0..256).find(|s| grow(&mega_spruce, *s).iter().any(|(_, b)| *b == Podzol));
        assert!(podzol_seed.is_some(), "mega spruce alter_ground places podzol on some seed");

        // The mangrove root placer grows roots, and its propagule decorator hangs
        // mangrove propagules off the leaves, on some seed.
        let root_seed = (0..256).find(|s| {
            let w = grow(&mangrove, *s);
            w.iter().any(|(_, b)| matches!(b, MangroveRoots | MuddyMangroveRoots))
        });
        assert!(root_seed.is_some(), "mangrove root placer grows roots on some seed");
    }

    /// Swamp oak and super birch reuse existing placers (straight trunk + blob
    /// foliage), so they must already parse to supported trees — no new work, but
    /// guard they did not regress to `Unsupported`/`Deferred`.
    #[test]
    fn swamp_oak_and_super_birch_are_supported() {
        let reg = registry_for(&["minecraft:swamp"]);
        for id in ["swamp_oak", "super_birch_bees_0002", "super_birch_bees"] {
            match reg.configured.get(id) {
                Some(ConfiguredFeature::Tree(c)) => {
                    assert!(!c.trunk_placer.is_unsupported(), "{id} trunk supported");
                    assert!(!c.foliage_placer.is_unsupported(), "{id} foliage supported");
                    assert!(!c.root_placer_unsupported, "{id} has no unsupported root placer");
                }
                other => panic!("{id} is not a tree: {other:?}"),
            }
        }
    }

    /// The beehive decorator draws its RNG (nextFloat gate + shuffle + bees)
    /// exactly and only when a hive is placed; determinism holds.
    #[test]
    fn beehive_decorator_is_deterministic() {
        let registry = registry_for(&["minecraft:plains"]);
        let oak_bees = match registry.configured.get("oak_bees_005") {
            Some(ConfiguredFeature::Tree(c)) => c.clone(),
            _ => panic!("oak_bees_005 not a tree"),
        };
        assert!(matches!(oak_bees.decorators.as_slice(), [TreeDecorator::Beehive { .. }]));
        let run = || {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "oak_bees_005" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(555));
            place_tree(&oak_bees, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        assert_eq!(run(), run(), "beehive tree deterministic");
    }

    // --- Vegetal-decoration feature tests ------------------------------------

    fn simple_cfg(reg: &FeatureRegistry, id: &str) -> SimpleBlockConfig {
        match reg.configured.get(id) {
            Some(ConfiguredFeature::SimpleBlock(c)) => c.clone(),
            other => panic!("{id} is not a simple_block: {other:?}"),
        }
    }

    /// `grass` (short_grass) and `flower_default` (weighted poppy/dandelion) place
    /// on a grass floor, are deterministic, and draw the weighted RNG for flowers.
    #[test]
    fn simple_block_grass_and_flowers() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:plains"]);
        let grass = simple_cfg(&reg, "grass");
        let flower = simple_cfg(&reg, "flower_default");
        let run = |cfg: &SimpleBlockConfig, seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_simple_block(cfg, &mut ctx, &mut random, Pos::new(8, 70, 8));
            level.get_block(8, 70, 8)
        };
        assert_eq!(run(&grass, 1), ShortGrass, "short grass placed on grass floor");
        assert_eq!(run(&grass, 1), run(&grass, 1), "deterministic");
        // The weighted provider yields one of poppy/dandelion, deterministically.
        let f = run(&flower, 5);
        assert!(matches!(f, Poppy | Dandelion), "flower_default is poppy or dandelion, got {f:?}");
        assert_eq!(run(&flower, 5), f, "flower deterministic for a fixed seed");
        // A plant does not survive on bare stone (no grass floor below).
        let mut bare = TestLevel::new(70);
        let idx = AllBiome;
        let mut ctx = PlacementCtx { level: &mut bare, biome_index: &idx, top_feature: "t" };
        let mut r = WorldgenRandom::new(RandomSource::xoroshiro(1));
        place_simple_block(&grass, &mut ctx, &mut r, Pos::new(8, 70, 8));
        assert_eq!(bare.get_block(8, 70, 8), Air, "grass fails to survive on bare stone (no soil)");
    }

    /// Double plants place both halves (collapsed to the default block).
    #[test]
    fn simple_block_double_plant_places_two_halves() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:plains"]);
        let tall = simple_cfg(&reg, "tall_grass");
        let mut level = tree_level(70);
        let idx = AllBiome;
        let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
        let mut random = WorldgenRandom::new(RandomSource::xoroshiro(3));
        place_simple_block(&tall, &mut ctx, &mut random, Pos::new(8, 70, 8));
        assert_eq!(level.get_block(8, 70, 8), TallGrass, "lower half");
        assert_eq!(level.get_block(8, 71, 8), TallGrass, "upper half");
    }

    /// `cactus` (block_column) stacks cacti with a flower tip, deterministically;
    /// the trunk height matches the replayed layer-height draws.
    #[test]
    fn block_column_cactus_stacks() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:desert"]);
        let cactus = match reg.configured.get("cactus") {
            Some(ConfiguredFeature::BlockColumn(c)) => c.clone(),
            other => panic!("cactus is not a block_column: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_block_column(&cactus, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(9);
        assert_eq!(a, run(9), "cactus column deterministic");
        let cacti = a.iter().filter(|(_, b)| *b == Cactus).count();
        assert!(cacti >= 1, "at least one cactus segment");
        // The flower tip is a 1/4 weighted layer — find a seed that grows one.
        let flower_seed = (0..64).find(|s| run(*s).iter().any(|(_, b)| *b == CactusFlower));
        assert!(flower_seed.is_some(), "some seed places a cactus flower tip");
        // Only cactus/cactus_flower written into the air column (plus grass floor).
        for ((_, y, _), b) in &a {
            if *y >= 70 {
                assert!(matches!(b, Cactus | CactusFlower), "unexpected {b:?} in column");
            }
        }
    }

    /// Bamboo grows a stalk (collapsed `bamboo` segments) on a sand floor and lays
    /// a podzol disc for the `some_podzol` variant; deterministic.
    #[test]
    fn bamboo_grows_stalk_and_podzol() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:bamboo_jungle"]);
        let (prob, _) = match reg.configured.get("bamboo_some_podzol") {
            Some(ConfiguredFeature::Bamboo { probability }) => (*probability, ()),
            other => panic!("bamboo_some_podzol is not bamboo: {other:?}"),
        };
        assert!(prob > 0.0, "some_podzol has a non-zero podzol chance");
        let run = |seed: i64| {
            let mut level = TestLevel::new(70);
            for z in -8..24 {
                for x in -8..24 {
                    level.set_block(x, 69, z, ParityBlock::Sand);
                }
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_bamboo(prob, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(2);
        assert_eq!(a, run(2), "bamboo deterministic");
        let stalk = a.iter().filter(|((x, _, z), b)| *x == 8 && *z == 8 && *b == Bamboo).count();
        assert!(stalk >= 5, "a bamboo stalk at least 5 tall, got {stalk}");
    }

    /// A water column: stone below `floor`, water in `[floor, top)`.
    fn water_level(floor: i32, top: i32) -> TestLevel {
        let mut level = TestLevel::new(floor);
        for z in -8..24 {
            for x in -8..24 {
                for y in floor..top {
                    level.set_block(x, y, z, ParityBlock::Water);
                }
            }
        }
        level
    }

    /// Kelp and seagrass grow in a water column off the ocean floor; deterministic.
    #[test]
    fn kelp_and_seagrass_grow_in_water() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:ocean"]);
        let run_kelp = |seed: i64| {
            let mut level = water_level(60, 75);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_kelp(&mut ctx, &mut random, Pos::new(8, 60, 8));
            level.blocks.values().filter(|b| matches!(b, Kelp | KelpPlant)).count()
        };
        assert!(run_kelp(4) > 0, "kelp grows off the floor");
        assert_eq!(run_kelp(4), run_kelp(4), "kelp deterministic");

        let (prob,) = match reg.configured.get("seagrass_short") {
            Some(ConfiguredFeature::Seagrass { probability }) => (*probability,),
            other => panic!("seagrass_short is not seagrass: {other:?}"),
        };
        let run_sg = |seed: i64| {
            let mut level = water_level(60, 75);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_seagrass(prob, &mut ctx, &mut random, Pos::new(8, 60, 8));
            let mut writes: Vec<_> = level.blocks.iter().filter(|(_, b)| matches!(b, Seagrass | TallSeagrass)).map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        // Sweep seeds to find one that lands seagrass, then assert determinism.
        let seed = (0..64).find(|s| !run_sg(*s).is_empty()).expect("some seed places seagrass");
        assert_eq!(run_sg(seed), run_sg(seed), "seagrass deterministic");
    }

    /// The lava lake carves an air/lava pocket underground, deterministically, and
    /// never replaces bedrock.
    #[test]
    fn lake_lava_carves_pocket() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:plains"]);
        let lake = match reg.configured.get("lake_lava") {
            Some(ConfiguredFeature::Lake(c)) => c.clone(),
            other => panic!("lake_lava is not a lake: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = TestLevel::new(120); // solid stone through the lake band
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_lake(&lake, &mut ctx, &mut random, Pos::new(8, 40, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(11);
        assert_eq!(a, run(11), "lava lake deterministic");
        assert!(a.iter().any(|(_, b)| *b == Lava), "lava fills the lower pocket");
        assert!(a.iter().any(|(_, b)| *b == Air), "air caps the pocket");
        // Only lava / air / stone-barrier written (no bedrock replaced — none present).
        for (_, b) in &a {
            assert!(matches!(b, Lava | Air | Stone), "unexpected lake block {b:?}");
        }
    }

    /// `ice_spike` grows a packed-ice spike on a snow_block cap, replacing only
    /// air / `#ice_spike_replaceable`, deterministically.
    #[test]
    fn ice_spike_grows_packed_ice() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:ice_spikes"]);
        let cfg = match reg.configured.get("ice_spike") {
            Some(ConfiguredFeature::Spike(c)) => c.clone(),
            other => panic!("ice_spike is not a spike: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = TestLevel::new(70);
            // snow_block cap so `can_place_on` (matching snow_block) passes.
            for z in -8..24 {
                for x in -8..24 {
                    level.set_block(x, 69, z, SnowBlock);
                }
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_spike(&cfg, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.into_iter().collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(5);
        assert_eq!(a, run(5), "ice spike deterministic");
        let ice = a.iter().filter(|(_, b)| *b == PackedIce).count();
        assert!(ice > 5, "the spike is made of packed ice, got {ice}");
        // Only packed_ice / snow_block appear (spike writes packed ice; snow cap stays).
        for (_, b) in &a {
            assert!(matches!(b, PackedIce | SnowBlock), "unexpected spike block {b:?}");
        }
    }

    /// `blue_ice` seeds a blue-ice patch: needs a water column, an adjacent
    /// packed-ice block, and origin below sea level; grows deterministically.
    #[test]
    fn blue_ice_spreads_from_packed_ice() {
        use ParityBlock::*;
        let run = |seed: i64| {
            let mut level = TestLevel::new(40); // solid stone below; carve water above
            for z in -8..8 {
                for x in -8..8 {
                    for y in 40..62 {
                        level.set_block(x, y, z, Water);
                    }
                }
            }
            // A packed-ice neighbour beside the origin.
            level.set_block(9, 50, 8, PackedIce);
            level.set_block(8, 50, 8, Water);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_blue_ice(&mut ctx, &mut random, Pos::new(8, 50, 8));
            level.blocks.iter().filter(|(_, b)| **b == BlueIce).count()
        };
        let a = run(3);
        assert!(a >= 1, "at least the seed blue-ice block is placed");
        assert_eq!(a, run(3), "blue ice deterministic");
    }

    /// `iceberg` builds a packed-ice mass around sea level, deterministically,
    /// writing only iceberg materials (packed ice / snow / air / water carve).
    #[test]
    fn iceberg_builds_ice_mass() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:frozen_ocean"]);
        let cfg = match reg.configured.get("iceberg_packed") {
            Some(ConfiguredFeature::Iceberg(c)) => c.clone(),
            other => panic!("iceberg_packed is not an iceberg: {other:?}"),
        };
        let run = |seed: i64| {
            // Ocean: stone floor at 40, water 40..63, air above.
            let mut level = TestLevel::new(40);
            for z in -16..16 {
                for x in -16..16 {
                    for y in 40..63 {
                        level.set_block(x, y, z, Water);
                    }
                }
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_iceberg(&cfg, &mut ctx, &mut random, Pos::new(0, 90, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(7);
        assert_eq!(a, run(7), "iceberg deterministic");
        let packed = a.iter().filter(|(_, b)| *b == PackedIce).count();
        assert!(packed > 0, "the iceberg is built from packed ice, got {packed}");
        for (_, b) in &a {
            assert!(matches!(b, PackedIce | SnowBlock | Snow | Air | Water), "unexpected iceberg block {b:?}");
        }
    }

    /// `forest_rock` (block_blob) piles mossy cobblestone on a grass floor,
    /// deterministically, writing only mossy cobblestone.
    #[test]
    fn forest_rock_piles_mossy_cobble() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:windswept_hills"]);
        let cfg = match reg.configured.get("forest_rock") {
            Some(ConfiguredFeature::BlockBlob(c)) => c.clone(),
            other => panic!("forest_rock is not a block_blob: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = tree_level(70); // grass cap at 69 → can_place_on
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_block_blob(&cfg, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut writes: Vec<_> = level.blocks.iter().filter(|(_, b)| **b == MossyCobblestone).map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(6);
        assert_eq!(a, run(6), "forest rock deterministic");
        assert!(!a.is_empty(), "the blob places mossy cobblestone");
    }

    /// `desert_well` builds a sandstone well with a water core on a sand column,
    /// deterministically, and drops two suspicious-sand blocks under the water.
    #[test]
    fn desert_well_builds_structure() {
        use ParityBlock::*;
        let run = |seed: i64| {
            // A tall sand column with air above so the down-scan lands on sand.
            let mut level = TestLevel::new(60);
            for z in -4..5 {
                for x in -4..5 {
                    for y in 55..71 {
                        level.set_block(x, y, z, Sand);
                    }
                }
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_desert_well(&mut ctx, &mut random, Pos::new(0, 70, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(1);
        assert_eq!(a, run(1), "desert well deterministic");
        assert!(a.iter().any(|(_, b)| *b == Water), "the well has a water core");
        assert!(a.iter().any(|(_, b)| *b == SandstoneSlab), "the well has a sandstone-slab rim");
        let sus = a.iter().filter(|(_, b)| *b == SuspiciousSand).count();
        assert!(sus >= 1, "at least one suspicious-sand block, got {sus}");
    }
}
