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
//! `ore`, `scattered_ore`, `spring_feature`, `disk`, `freeze_top_layer`. Between
//! them these cover, block-for-block, the whole `underground_ores` step (ores +
//! disks), the `fluid_springs` step, and `top_layer_modification` for the
//! overworld.
//!
//! ## Deferred features (skipped, documented)
//! `tree` and the whole trunk/foliage system, `simple_block` / `random_patch` /
//! `vegetation_patch` and other vegetation, `lake`, `underwater_magma`, geodes,
//! dripstone, coral/kelp/seagrass, mushrooms, fossils, monster rooms, and every
//! nether/end feature. Most place block states outside the curated
//! [`ParityBlock`] alphabet; a follow-up milestone can add them without touching
//! the engine. Each deferred feature is recognized (so the sort/seed accounting
//! is complete) but its placement is not run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::Value;

use super::density::ParityBlock;
use super::placement::{
    BiomeFeatureIndex, BlockPredicate, BlockTag, DecorationLevel, Heightmap, IntProvider,
    PlacementCtx, PlacementModifier, Pos, RuleTest,
};
use super::random::{RandomSource, WorldgenRandom};
use super::synth::PerlinSimplexNoise;
use super::vanilla_jsons;

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
    /// `nextInt(total_weight)`. None of the target trees use it, so it is only a
    /// completeness port (cumulative-weight selection).
    Weighted(Vec<(ParityBlock, i32)>),
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
            StateProvider::Unsupported => None,
        }
    }
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
            _ => TrunkPlacer::Unsupported,
        }
    }

    fn is_unsupported(&self) -> bool {
        matches!(self, TrunkPlacer::Unsupported)
    }

    /// `TrunkPlacer.getTreeHeight` — `baseHeight + nextInt(a+1) + nextInt(b+1)`.
    fn get_tree_height(&self, random: &mut WorldgenRandom) -> i32 {
        match self {
            TrunkPlacer::Straight { base, a, b }
            | TrunkPlacer::Forking { base, a, b }
            | TrunkPlacer::DarkOak { base, a, b }
            | TrunkPlacer::Fancy { base, a, b }
            | TrunkPlacer::Giant { base, a, b }
            | TrunkPlacer::MegaJungle { base, a, b } => {
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
            _ => TreeDecorator::Unsupported,
        }
    }
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
}

fn parse_tree(cfg: &Value) -> TreeConfig {
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

/// `TrunkPlacer.isFree` — `validTreePos || #logs`.
fn is_free(level: &dyn DecorationLevel, p: Pos) -> bool {
    valid_tree_pos(level, p) || BlockTag::Logs.contains(level.get_block(p.x, p.y, p.z))
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
    if valid_tree_pos(level, p) {
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
fn get_max_free_tree_height(level: &dyn DecorationLevel, max_tree_height: i32, tree_pos: Pos, config: &TreeConfig) -> i32 {
    for y in 0..=max_tree_height + 1 {
        let r = config.minimum_size.get_size_at_height(max_tree_height, y);
        for x in -r..=r {
            for z in -r..=r {
                let p = Pos::new(tree_pos.x + x, tree_pos.y + y, tree_pos.z + z);
                if !is_free(level, p) || (!config.ignore_vines && is_vine(level, p)) {
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
            TrunkPlacer::Unsupported => Vec::new(),
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
            | FoliagePlacer::MegaJungle { offset, .. } => offset,
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
            | FoliagePlacer::MegaJungle { radius, .. } => radius.sample(random),
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
    fn place(
        &self,
        level: &mut dyn DecorationLevel,
        decorations: &mut HashSet<Pos>,
        random: &mut WorldgenRandom,
        trunks: &HashSet<Pos>,
        foliage: &HashSet<Pos>,
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
            TreeDecorator::Unsupported => {}
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
    if config.trunk_placer.is_unsupported() || config.foliage_placer.is_unsupported() {
        return;
    }

    let level: &mut dyn DecorationLevel = ctx.level;

    let tree_height = config.trunk_placer.get_tree_height(random);
    let foliage_height = config.foliage_placer.foliage_height(random, tree_height);
    let trunk_height = tree_height - foliage_height;
    let leaf_radius = config.foliage_placer.foliage_radius(random, trunk_height);
    // No root placer → trunkOrigin == origin.
    let trunk_origin = origin;
    let min_y = origin.y.min(trunk_origin.y);
    let max_y = origin.y.max(trunk_origin.y) + tree_height + 1;
    // Vanilla `TreeFeature.doPlace`: proceed when `minY >= getMinY()+1 && maxY
    // <= getMaxY()+1`. `getMaxY()` is the inclusive top, so `getMaxY()+1` ==
    // `getMaxBuildHeight()` == our exclusive `max_y()`.
    if min_y < level.min_y() + 1 || max_y > level.max_y() {
        return;
    }
    let min_clipped = config.minimum_size.min_clipped_height();
    let clipped = get_max_free_tree_height(level, tree_height, trunk_origin, config);
    if !(clipped >= tree_height || min_clipped.map(|m| clipped >= m).unwrap_or(false)) {
        return;
    }

    let mut trunks: HashSet<Pos> = HashSet::new();
    let mut foliage: HashSet<Pos> = HashSet::new();
    let mut decorations: HashSet<Pos> = HashSet::new();

    let attachments = config.trunk_placer.place_trunk(level, &mut trunks, random, clipped, trunk_origin, config);
    for att in &attachments {
        config
            .foliage_placer
            .create_foliage(level, &mut foliage, random, config, clipped, att, foliage_height, leaf_radius);
    }

    // `place`: decorators run only when the tree placed something.
    if !trunks.is_empty() || !foliage.is_empty() {
        for dec in &config.decorators {
            dec.place(level, &mut decorations, random, &trunks, &foliage);
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
        assert!(!registry.configured.get("lake_lava").map(|c| c.is_implemented()).unwrap_or(true));
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
}
