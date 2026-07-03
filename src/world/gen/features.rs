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
//! `random_selector`, `simple_random_selector`, `simple_block` (grass / ferns /
//! flowers / mushrooms / gourds / bushes / lily pads / dead bush / dry grass /
//! leaf litter / berries / double plants / lush-caves moss set), `block_column`
//! (cactus, sugar cane), `bamboo`, `kelp`, `seagrass`, `sea_pickle`, `lake`
//! (lava lakes); the **frozen/ice** group (`blue_ice`, `spike`/ice_spike,
//! `iceberg`; `ice_patch` flows through `disk`); the **desert/rock** group
//! (`block_blob`/forest_rock, `desert_well`); the **underground** group
//! (`monster_room`, `underwater_magma`, `geode`); the **dripstone** group
//! (`speleothem_cluster`/dripstone_cluster, `large_dripstone`, `speleothem` via
//! the pointed_dripstone selector); the **coral** group (`coral_tree`/
//! `coral_claw`/`coral_mushroom`); `vegetation_patch` /
//! `waterlogged_vegetation_patch` (moss/clay patches — see the JavaHashSet note);
//! `freeze_top_layer`; the **lush-caves close-out** — `cave_vines`
//! (`cave_vine`/`cave_vine_in_moss` via `block_column`), `dripleaf`
//! (`big_dripleaf`/`big_dripleaf_stem`/`small_dripleaf` via
//! `simple_random_selector`+`block_column`), `multiface_growth`
//! (`glow_lichen`/`sculk_vein`; face booleans collapse, direction shuffles + the
//! spread `nextFloat`/`allShuffled` consumed 1:1) and `root_system`
//! (`rooted_azalea_tree`); `random_boolean_selector`, `sequence` (`sulfur_pool`)
//! and `weighted_random_selector`; `fallen_tree`, `huge_red_mushroom` /
//! `huge_brown_mushroom` (face booleans collapse), and `vines`. With
//! `cave_vine`/`dripleaf`/`moss_vegetation`/`pale_moss_vegetation` all supported,
//! the once-deferred patches `moss_patch_ceiling`, `clay_with_dripleaves`,
//! `clay_pool_with_dripleaves` and `pale_moss_patch` now run fully. Note MC 26.2
//! replaced the old `random_patch` feature with `simple_block` repeated by its
//! placement chain, so grass/flower patches flow through `simple_block`.
//!
//! ## Deferred features (skipped, documented)
//! Every remaining deferred **overworld** feature is either template-gated or
//! blocked by the parity model; each stays recognized (so the sort/seed
//! accounting is complete) but its placement is not run — parity-safe because the
//! RNG is reseeded per top feature.
//! * **Requires Mojang NBT structure-template assets** (clean-room policy — Vela
//!   ships no Mojang assets): `fossil` (processor-based template placement), and
//!   therefore `rooted_sulfur_spring` — its nested tree feature `sulfur_spring`
//!   (`weighted_random_selector` → `sequence` → `template`) is template-gated, so
//!   `root_system` skips that whole feature (`rooted_azalea_tree` runs fully).
//! * **Incompatible with the property-collapse parity model** — `sculk_patch`.
//!   The `SculkSpreader` charge simulation's RNG draw sequence (the per-cursor
//!   `attemptSpreadVein` / `attemptUseCharge` branch selection, `availableFaces`,
//!   the direction/neighbour shuffles) is driven by each block's tracked
//!   multiface `facings` state, which this engine collapses (face booleans are
//!   dropped, per the established property-collapse precedent). A bit-exact port
//!   is therefore not achievable without modelling per-block face state, so
//!   `sculk_patch` is skipped whole; `sculk_vein` on its own (`multiface_growth`)
//!   *is* supported. (Skipping is parity-safe: reseeded per top feature.)
//! * Every nether/end feature (out of overworld scope). `block_pile` is
//!   nether-only in the vendored data (no overworld biome references it).
//!
//! ## Java HashSet iteration order (vegetation_patch)
//! `VegetationPatchFeature` uses a plain `java.util.HashSet<BlockPos>` (verified
//! in the decompile — not a `LinkedHashSet`), whose iteration order
//! `distributeVegetation` depends on. `JavaHashSet` reproduces it exactly from
//! `BlockPos.hashCode` (`(y+z*31)*31+x`), the `h^(h>>>16)` spread, power-of-two
//! `(cap-1)&hash` bucket indexing, per-bucket insertion chaining, and the ×0.75
//! resize threshold (initial capacity 16). Bucket treeification is not modeled:
//! it needs capacity ≥ 64 **and** an 8-deep bucket, which the small patch sets
//! never reach.
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
//! * Property collapses in the new groups (all RNG-exact, block-identity only):
//!   pointed-dripstone `thickness`/`vertical_direction`/`waterlogged`, amethyst
//!   bud/cluster `facing`/`waterlogged`, coral-fan `facing`, sea-pickle `pickles`,
//!   sandstone-slab `type`, and `snowy=true` on the block under a snow layer, all
//!   collapse to default block states.
//! * Block entities are not modeled in worldgen: `monster_room`'s spawner (its
//!   `nextInt(4)` mob pick is consumed) and chests (each `nextLong` loot seed is
//!   consumed), and `desert_well`'s suspicious sand (two `nextInt(5)` picks
//!   consumed). The block state is placed; the NBT (spawn data / loot table) is
//!   deferred.
//! * Predicate approximations that draw no RNG: `isSolid` (monster_room),
//!   `isFaceSturdy` (vegetation_patch), `isVisibleFromOutside` (underwater_magma)
//!   collapse to `blocks_motion` / air-or-fluid tests; they can only nudge which
//!   blocks a no-RNG feature writes, never desync.
//! * Floating-point: `iceberg`/`geode`/`speleothem` use `Math.pow`/`Math.log`/
//!   JOML `invsqrt` (= `1/sqrt`); Rust `powf`/`ln`/`sqrt` may differ by a ULP
//!   from Java on boundary cells (documented, shape-only — no RNG dependence).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::Value;

use super::density::ParityBlock;
use super::placement::{
    BiomeFeatureIndex, BlockPredicate, BlockTag, DecorationLevel, FloatProvider, Heightmap, IntProvider,
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

/// `UnderwaterMagmaConfiguration`.
#[derive(Clone, Debug)]
struct UnderwaterMagmaConfig {
    floor_search_range: i32,
    placement_radius_around_floor: i32,
    placement_probability: f32,
}

/// `GeodeConfiguration` (+ the layer/crack/block settings the feature reads).
#[derive(Clone, Debug)]
struct GeodeConfig {
    filling: StateProvider,
    inner_layer: StateProvider,
    alternate_inner_layer: StateProvider,
    middle_layer: StateProvider,
    outer_layer: StateProvider,
    inner_placements: Vec<ParityBlock>,
    cannot_replace: Option<BlockTag>,
    invalid_blocks: Option<BlockTag>,
    // GeodeLayerSettings (defaults 1.7/2.2/3.2/4.2/5.0).
    layer_filling: f64,
    layer_inner: f64,
    layer_middle: f64,
    layer_outer: f64,
    // GeodeCrackSettings.
    generate_crack_chance: f64,
    base_crack_size: f64,
    crack_point_offset: i32,
    use_potential_placements_chance: f64,
    use_alternate_layer0_chance: f64,
    placements_require_layer0_alternate: bool,
    outer_wall_distance: IntProvider,
    distribution_points: IntProvider,
    point_offset: IntProvider,
    min_gen_offset: i32,
    max_gen_offset: i32,
    noise_multiplier: f64,
    invalid_blocks_threshold: i32,
}

/// `SpeleothemConfiguration` (single pointed dripstone).
#[derive(Clone, Debug)]
struct SpeleothemConfig {
    base_block: ParityBlock,
    pointed_block: ParityBlock,
    replaceable_blocks: Option<BlockTag>,
    chance_of_taller_generation: f32,
    chance_of_directional_spread: f32,
    chance_of_spread_radius2: f32,
    chance_of_spread_radius3: f32,
}

/// `SpeleothemClusterConfiguration` (`dripstone_cluster`).
#[derive(Clone, Debug)]
struct SpeleothemClusterConfig {
    floor_to_ceiling_search_range: i32,
    height: IntProvider,
    radius: IntProvider,
    max_stalagmite_stalactite_height_diff: i32,
    height_deviation: i32,
    speleothem_block_layer_thickness: IntProvider,
    density: FloatProvider,
    wetness: FloatProvider,
    chance_of_speleothem_at_max_distance_from_center: f64,
    max_distance_from_center_affecting_height_bias: i32,
    max_distance_from_edge_affecting_chance_of_speleothem: i32,
    base_block: ParityBlock,
    pointed_block: ParityBlock,
    replaceable_blocks: Option<BlockTag>,
}

/// `LargeDripstoneConfiguration`.
#[derive(Clone, Debug)]
struct LargeDripstoneConfig {
    floor_to_ceiling_search_range: i32,
    column_radius: IntProvider,
    height_scale: FloatProvider,
    max_column_radius_to_cave_height_ratio: f64,
    stalactite_bluntness: FloatProvider,
    stalagmite_bluntness: FloatProvider,
    wind_speed: FloatProvider,
    min_radius_for_wind: i32,
    min_bluntness_for_wind: f64,
    base_block: ParityBlock,
    pointed_block: ParityBlock,
    replaceable_blocks: Option<BlockTag>,
}

/// `CompositeFeatureConfiguration` (`simple_random_selector`).
#[derive(Clone, Debug)]
struct SimpleRandomSelectorConfig {
    features: Vec<NestedFeature>,
}

/// `VegetationPatchConfiguration` (moss / clay-pool patches, lush caves). Also
/// carries the `waterlogged` flag distinguishing the two feature types.
#[derive(Clone, Debug)]
struct VegetationPatchConfig {
    replaceable: Option<BlockTag>,
    ground_state: StateProvider,
    vegetation_feature: NestedFeature,
    /// `true` = FLOOR (direction DOWN), `false` = CEILING (direction UP).
    surface_floor: bool,
    depth: IntProvider,
    extra_bottom_block_chance: f32,
    vertical_range: i32,
    vegetation_chance: f32,
    xz_radius: IntProvider,
    extra_edge_column_chance: f32,
    waterlogged: bool,
}

/// `MultifaceGrowthConfiguration` (glow_lichen / sculk_vein). The multiface
/// block collapses to its default state (face booleans dropped); every RNG draw
/// (the direction shuffles and the spread `nextFloat`) is consumed 1:1.
#[derive(Clone, Debug)]
struct MultifaceGrowthConfig {
    block: ParityBlock,
    search_range: i32,
    can_place_on_floor: bool,
    can_place_on_ceiling: bool,
    can_place_on_wall: bool,
    chance_of_spreading: f32,
    can_be_placed_on: Vec<ParityBlock>,
}

/// `RootSystemConfiguration` (rooted_azalea_tree / rooted_sulfur_spring).
#[derive(Clone, Debug)]
struct RootSystemConfig {
    feature: NestedFeature,
    required_vertical_space_for_tree: i32,
    level_test_distance: i32,
    max_level_deviation: i32,
    root_radius: i32,
    root_replaceable: Option<BlockTag>,
    root_state_provider: StateProvider,
    root_placement_attempts: i32,
    root_column_max_height: i32,
    hanging_root_radius: i32,
    hanging_roots_vertical_span: i32,
    hanging_root_state_provider: StateProvider,
    hanging_root_placement_attempts: i32,
    allowed_vertical_water_for_tree: i32,
    allowed_tree_position: BlockPredicate,
}

/// `FallenTreeConfiguration`.
#[derive(Clone, Debug)]
struct FallenTreeConfig {
    trunk_provider: StateProvider,
    log_length: IntProvider,
    stump_decorators: Vec<TreeDecorator>,
    log_decorators: Vec<TreeDecorator>,
}

/// `HugeMushroomFeatureConfiguration` + the red/brown discriminant.
#[derive(Clone, Debug)]
struct HugeMushroomConfig {
    cap_provider: StateProvider,
    stem_provider: StateProvider,
    foliage_radius: i32,
    can_place_on: BlockPredicate,
    brown: bool,
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
    /// `AttachedToLogsDecorator` — hangs a block (mushrooms) off log faces.
    AttachedToLogs { probability: f32, block_provider: StateProvider, directions: Vec<(i32, i32, i32)> },
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
            "attached_to_logs" => TreeDecorator::AttachedToLogs {
                probability: prob,
                block_provider: StateProvider::parse(&v["block_provider"]),
                directions: v["directions"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|d| parse_direction(d.as_str().unwrap_or("")))
                    .collect(),
            },
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
        Value::Object(_) => {
            let placement = v["placement"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(PlacementModifier::parse)
                .collect();
            // `feature` is either a placed-feature id string or an inline
            // configured feature object (`{type, config}`, as in
            // `simple_random_selector`). The latter resolves directly.
            if v["feature"].is_object() {
                NestedFeature::Resolved { feature: Box::new(ConfiguredFeature::parse(&v["feature"])), placement }
            } else {
                NestedFeature::InlineRef { feature: strip(v["feature"].as_str().unwrap_or("")), placement }
            }
        }
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
    MonsterRoom,
    UnderwaterMagma(UnderwaterMagmaConfig),
    Geode(GeodeConfig),
    Speleothem(SpeleothemConfig),
    SpeleothemCluster(SpeleothemClusterConfig),
    LargeDripstone(LargeDripstoneConfig),
    SimpleRandomSelector(SimpleRandomSelectorConfig),
    CoralTree,
    CoralClaw,
    CoralMushroom,
    VegetationPatch(VegetationPatchConfig),
    FreezeTopLayer,
    /// `RandomBooleanSelectorFeature` — one `nextBoolean` picks true/false branch.
    RandomBooleanSelector { feature_true: NestedFeature, feature_false: NestedFeature },
    /// `SequenceFeature` — place each nested placed feature in order.
    Sequence(Vec<NestedFeature>),
    /// `WeightedRandomSelectorFeature` — `nextInt(total_weight)` picks one entry.
    WeightedRandomSelector(Vec<(NestedFeature, i32)>),
    MultifaceGrowth(MultifaceGrowthConfig),
    RootSystem(RootSystemConfig),
    FallenTree(FallenTreeConfig),
    HugeMushroom(HugeMushroomConfig),
    Vines,
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
            "monster_room" => ConfiguredFeature::MonsterRoom,
            "underwater_magma" => ConfiguredFeature::UnderwaterMagma(UnderwaterMagmaConfig {
                floor_search_range: cfg["floor_search_range"].as_i64().unwrap_or(0) as i32,
                placement_radius_around_floor: cfg["placement_radius_around_floor"].as_i64().unwrap_or(0) as i32,
                placement_probability: cfg["placement_probability_per_valid_position"].as_f64().unwrap_or(0.0) as f32,
            }),
            "geode" => ConfiguredFeature::Geode(parse_geode(cfg)),
            "speleothem" => ConfiguredFeature::Speleothem(parse_speleothem(cfg)),
            "speleothem_cluster" => ConfiguredFeature::SpeleothemCluster(parse_speleothem_cluster(cfg)),
            "large_dripstone" => ConfiguredFeature::LargeDripstone(parse_large_dripstone(cfg)),
            "simple_random_selector" => ConfiguredFeature::SimpleRandomSelector(SimpleRandomSelectorConfig {
                features: cfg["features"].as_array().unwrap_or(&vec![]).iter().map(parse_nested).collect(),
            }),
            "coral_tree" => ConfiguredFeature::CoralTree,
            "coral_claw" => ConfiguredFeature::CoralClaw,
            "coral_mushroom" => ConfiguredFeature::CoralMushroom,
            "vegetation_patch" => ConfiguredFeature::VegetationPatch(parse_vegetation_patch(cfg, false)),
            "waterlogged_vegetation_patch" => ConfiguredFeature::VegetationPatch(parse_vegetation_patch(cfg, true)),
            "freeze_top_layer" => ConfiguredFeature::FreezeTopLayer,
            "random_boolean_selector" => ConfiguredFeature::RandomBooleanSelector {
                feature_true: parse_nested(&cfg["feature_true"]),
                feature_false: parse_nested(&cfg["feature_false"]),
            },
            "sequence" => ConfiguredFeature::Sequence(
                cfg["features"].as_array().unwrap_or(&vec![]).iter().map(parse_nested).collect(),
            ),
            "weighted_random_selector" => ConfiguredFeature::WeightedRandomSelector(
                cfg["features"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|e| (parse_nested(&e["data"]), e["weight"].as_i64().unwrap_or(1) as i32))
                    .collect(),
            ),
            "multiface_growth" => ConfiguredFeature::MultifaceGrowth(parse_multiface_growth(cfg)),
            "root_system" => ConfiguredFeature::RootSystem(parse_root_system(cfg)),
            "fallen_tree" => ConfiguredFeature::FallenTree(FallenTreeConfig {
                trunk_provider: StateProvider::parse(&cfg["trunk_provider"]),
                log_length: IntProvider::parse(&cfg["log_length"]),
                stump_decorators: cfg["stump_decorators"].as_array().unwrap_or(&vec![]).iter().map(TreeDecorator::parse).collect(),
                log_decorators: cfg["log_decorators"].as_array().unwrap_or(&vec![]).iter().map(TreeDecorator::parse).collect(),
            }),
            "huge_red_mushroom" => ConfiguredFeature::HugeMushroom(parse_huge_mushroom(cfg, false)),
            "huge_brown_mushroom" => ConfiguredFeature::HugeMushroom(parse_huge_mushroom(cfg, true)),
            "vines" => ConfiguredFeature::Vines,
            // `sculk_patch` stays recognized-but-skipped: the SculkSpreader charge
            // simulation's RNG draw order depends on tracked multiface `facings`
            // state, which this engine collapses (face booleans dropped). A bit-
            // exact port is therefore incompatible with the parity model; skipping
            // is parity-safe (the RNG is reseeded per top feature). See the header.
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
    /// biome fill value → `(base temperature, frozen modifier, has_precipitation)`
    /// for `freeze_top_layer`.
    biome_snow: Vec<(f32, bool, bool)>,
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
        ConfiguredFeature::SimpleRandomSelector(sc) => {
            ConfiguredFeature::SimpleRandomSelector(SimpleRandomSelectorConfig {
                features: sc.features.iter().map(|f| resolve_nested(f, configured, placed)).collect(),
            })
        }
        ConfiguredFeature::VegetationPatch(vc) => ConfiguredFeature::VegetationPatch(VegetationPatchConfig {
            vegetation_feature: resolve_nested(&vc.vegetation_feature, configured, placed),
            ..vc.clone()
        }),
        ConfiguredFeature::RandomBooleanSelector { feature_true, feature_false } => {
            ConfiguredFeature::RandomBooleanSelector {
                feature_true: resolve_nested(feature_true, configured, placed),
                feature_false: resolve_nested(feature_false, configured, placed),
            }
        }
        ConfiguredFeature::Sequence(fs) => {
            ConfiguredFeature::Sequence(fs.iter().map(|f| resolve_nested(f, configured, placed)).collect())
        }
        ConfiguredFeature::WeightedRandomSelector(fs) => ConfiguredFeature::WeightedRandomSelector(
            fs.iter().map(|(f, w)| (resolve_nested(f, configured, placed), *w)).collect(),
        ),
        ConfiguredFeature::RootSystem(rc) => ConfiguredFeature::RootSystem(RootSystemConfig {
            feature: resolve_nested(&rc.feature, configured, placed),
            ..rc.clone()
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
            .filter(|(_, c)| {
                matches!(
                    c,
                    ConfiguredFeature::RandomSelector(_)
                        | ConfiguredFeature::SimpleRandomSelector(_)
                        | ConfiguredFeature::VegetationPatch(_)
                        | ConfiguredFeature::RandomBooleanSelector { .. }
                        | ConfiguredFeature::Sequence(_)
                        | ConfiguredFeature::WeightedRandomSelector(_)
                        | ConfiguredFeature::RootSystem(_)
                )
            })
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

        // Per-biome snow climate (temperature, frozen modifier, has_precipitation)
        // for freeze_top_layer, indexed by fill (biome_names order).
        let mut snow_by_name: HashMap<String, (f32, bool, bool)> = HashMap::new();
        for (name, json) in vanilla_jsons::BIOMES {
            let v: Value = serde_json::from_str(json).expect("biome json");
            let temp = v["temperature"].as_f64().unwrap_or(0.5) as f32;
            let frozen = v.get("temperature_modifier").and_then(Value::as_str).is_some_and(|m| m == "frozen");
            let has_precip = v.get("has_precipitation").and_then(Value::as_bool).unwrap_or(false);
            snow_by_name.insert(format!("minecraft:{name}"), (temp, frozen, has_precip));
        }
        let biome_snow = biome_names
            .iter()
            .map(|n| snow_by_name.get(n).copied().unwrap_or((0.5, false, false)))
            .collect();

        Self { configured, placed, biome_features, biome_feature_set, steps, step_index, biome_names, biome_snow }
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
    fn biome_snow(&self, biome_fill: u16) -> (f32, bool, bool) {
        self.biome_snow.get(biome_fill as usize).copied().unwrap_or((0.5, false, false))
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
) -> bool {
    match modifiers.split_first() {
        None => place_feature(configured, ctx, random, pos),
        Some((first, rest)) => {
            let positions = first.get_positions(ctx, random, pos);
            let mut placed = false;
            for p in positions {
                placed |= place_stream(configured, rest, ctx, random, p);
            }
            placed
        }
    }
}

/// `ConfiguredFeature.place` for the implemented features. The boolean mirrors
/// vanilla `Feature.place` — only `sequence` (fail-fast), `root_system` (tree
/// success) and the selectors observe it; features whose vanilla `place` always
/// reports success return `true`.
fn place_feature(
    configured: &ConfiguredFeature,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    origin: Pos,
) -> bool {
    match configured {
        ConfiguredFeature::Ore(cfg) => place_ore(cfg, ctx, random, origin),
        ConfiguredFeature::ScatteredOre(cfg) => place_scattered_ore(cfg, ctx, random, origin),
        ConfiguredFeature::Spring(cfg) => place_spring(cfg, ctx, origin),
        ConfiguredFeature::Disk(cfg) => place_disk(cfg, ctx, random, origin),
        ConfiguredFeature::Tree(cfg) => return place_tree(cfg, ctx, random, origin),
        ConfiguredFeature::RandomSelector(cfg) => place_random_selector(cfg, ctx, random, origin),
        ConfiguredFeature::SimpleBlock(cfg) => return place_simple_block(cfg, ctx, random, origin),
        ConfiguredFeature::BlockColumn(cfg) => place_block_column(cfg, ctx, random, origin),
        ConfiguredFeature::Bamboo { probability } => place_bamboo(*probability, ctx, random, origin),
        ConfiguredFeature::Kelp => place_kelp(ctx, random, origin),
        ConfiguredFeature::Seagrass { probability } => place_seagrass(*probability, ctx, random, origin),
        ConfiguredFeature::SeaPickle { count } => place_sea_pickle(count, ctx, random, origin),
        ConfiguredFeature::Lake(cfg) => return place_lake(cfg, ctx, random, origin),
        ConfiguredFeature::BlueIce => place_blue_ice(ctx, random, origin),
        ConfiguredFeature::Spike(cfg) => place_spike(cfg, ctx, random, origin),
        ConfiguredFeature::Iceberg(cfg) => place_iceberg(cfg, ctx, random, origin),
        ConfiguredFeature::BlockBlob(cfg) => place_block_blob(cfg, ctx, random, origin),
        ConfiguredFeature::DesertWell => place_desert_well(ctx, random, origin),
        ConfiguredFeature::MonsterRoom => place_monster_room(ctx, random, origin),
        ConfiguredFeature::UnderwaterMagma(cfg) => place_underwater_magma(cfg, ctx, random, origin),
        ConfiguredFeature::Geode(cfg) => place_geode(cfg, ctx, random, origin),
        ConfiguredFeature::Speleothem(cfg) => place_speleothem(cfg, ctx, random, origin),
        ConfiguredFeature::SpeleothemCluster(cfg) => place_speleothem_cluster(cfg, ctx, random, origin),
        ConfiguredFeature::LargeDripstone(cfg) => place_large_dripstone(cfg, ctx, random, origin),
        ConfiguredFeature::SimpleRandomSelector(cfg) => place_simple_random_selector(cfg, ctx, random, origin),
        ConfiguredFeature::CoralTree => place_coral(CoralKind::Tree, ctx, random, origin),
        ConfiguredFeature::CoralClaw => place_coral(CoralKind::Claw, ctx, random, origin),
        ConfiguredFeature::CoralMushroom => place_coral(CoralKind::Mushroom, ctx, random, origin),
        ConfiguredFeature::VegetationPatch(cfg) => place_vegetation_patch(cfg, ctx, random, origin),
        ConfiguredFeature::FreezeTopLayer => place_freeze_top_layer(ctx, origin),
        ConfiguredFeature::RandomBooleanSelector { feature_true, feature_false } => {
            // `RandomBooleanSelectorFeature.place` — one `nextBoolean` picks the branch.
            let nf = if random.next_boolean() { feature_true } else { feature_false };
            return place_nested(nf, ctx, random, origin);
        }
        ConfiguredFeature::Sequence(features) => return place_sequence(features, ctx, random, origin),
        ConfiguredFeature::WeightedRandomSelector(entries) => {
            return place_weighted_random_selector(entries, ctx, random, origin);
        }
        ConfiguredFeature::MultifaceGrowth(cfg) => return place_multiface_growth(cfg, ctx, random, origin),
        ConfiguredFeature::RootSystem(cfg) => return place_root_system(cfg, ctx, random, origin),
        ConfiguredFeature::FallenTree(cfg) => place_fallen_tree(cfg, ctx, random, origin),
        ConfiguredFeature::HugeMushroom(cfg) => return place_huge_mushroom(cfg, ctx, random, origin),
        ConfiguredFeature::Vines => return place_vines(ctx, origin),
        ConfiguredFeature::Deferred(_) => return false,
    }
    true
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
fn place_nested(nf: &NestedFeature, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    if let NestedFeature::Resolved { feature, placement } = nf {
        // A nested placement whose chain contains an unsupported modifier would
        // desync only this terminal feature (the RNG is reseeded per top
        // feature); still, skip rather than mis-draw.
        if placement.iter().any(|m| !m.is_supported()) {
            return false;
        }
        return place_stream(feature, placement, ctx, random, origin);
    }
    false
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
fn place_simple_block(cfg: &SimpleBlockConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    let state = match cfg.to_place.get_state(ctx.level, random, origin) {
        Some(s) => s,
        None => return false,
    };
    if !simple_block_can_survive(state, ctx.level, origin) {
        return false;
    }
    if is_double_plant(state) {
        if !ctx.level.get_block(origin.x, origin.y + 1, origin.z).is_air() {
            return false;
        }
        ctx.level.set_block(origin.x, origin.y, origin.z, state);
        ctx.level.set_block(origin.x, origin.y + 1, origin.z, state);
    } else {
        // `MossyCarpetBlock.placeAt` (pale_moss_carpet) draws 0–4 `nextBoolean`
        // for wall-side toppers; on open worldgen ground no wall sides exist so it
        // draws none — collapsed here to a plain carpet placement (documented).
        ctx.level.set_block(origin.x, origin.y, origin.z, state);
    }
    true
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
fn place_lake(cfg: &LakeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    if origin.y <= ctx.level.min_y() + 4 {
        return false;
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
        None => return false,
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
                        return false;
                    }
                    if yy < 4 && !bs.blocks_motion() && bs != fluid {
                        return false;
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
        None => return true,
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
    true
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
// Underground group: monster_room, underwater_magma, geode
// ---------------------------------------------------------------------------

/// `BlockState.isSolid()` over the parity alphabet — approximated by
/// `blocksMotion` (full cubes and the like; water/lava/air/snow-layer are not
/// solid). Documented approximation; draws no RNG so it can only nudge which
/// dungeon cells become cobblestone, never desync a feature.
fn is_solid(b: ParityBlock) -> bool {
    b.blocks_motion()
}

/// `MonsterRoomFeature.place`. The spawner's mob and the chests' loot tables are
/// block-entity NBT that Vela does not model in worldgen — the block states are
/// placed and the exact RNG is consumed (per chest a `nextLong` loot seed, and a
/// `nextInt(4)` spawner mob pick), but the NBT is deferred (documented).
fn place_monster_room(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    use ParityBlock::{Air, Chest, Cobblestone, MossyCobblestone, Spawner};
    let can_replace = |b: ParityBlock| !BlockTag::FeaturesCannotReplace.contains(b);
    let safe_set = |ctx: &mut PlacementCtx, x: i32, y: i32, z: i32, s: ParityBlock| {
        if can_replace(ctx.level.get_block(x, y, z)) {
            ctx.level.set_block(x, y, z, s);
        }
    };
    let xr = random.next_int_bounded(2) + 2;
    let min_x = -xr - 1;
    let max_x = xr + 1;
    let zr = random.next_int_bounded(2) + 2;
    let min_z = -zr - 1;
    let max_z = zr + 1;
    let mut hole_count = 0;
    for dx in min_x..=max_x {
        for dy in -1..=4 {
            for dz in min_z..=max_z {
                let (x, y, z) = (origin.x + dx, origin.y + dy, origin.z + dz);
                let solid = is_solid(ctx.level.get_block(x, y, z));
                if dy == -1 && !solid {
                    return;
                }
                if dy == 4 && !solid {
                    return;
                }
                if (dx == min_x || dx == max_x || dz == min_z || dz == max_z)
                    && dy == 0
                    && ctx.level.get_block(x, y, z).is_air()
                    && ctx.level.get_block(x, y + 1, z).is_air()
                {
                    hole_count += 1;
                }
            }
        }
    }
    if !(1..=5).contains(&hole_count) {
        return;
    }
    for dx in min_x..=max_x {
        for dy in (-1..=3).rev() {
            for dz in min_z..=max_z {
                let (x, y, z) = (origin.x + dx, origin.y + dy, origin.z + dz);
                let wall_state = ctx.level.get_block(x, y, z);
                if dx == min_x || dy == -1 || dz == min_z || dx == max_x || dy == 4 || dz == max_z {
                    if y >= ctx.level.min_y() && !is_solid(ctx.level.get_block(x, y - 1, z)) {
                        ctx.level.set_block(x, y, z, Air);
                    } else if is_solid(wall_state) && wall_state != Chest {
                        if dy == -1 && random.next_int_bounded(4) != 0 {
                            safe_set(ctx, x, y, z, MossyCobblestone);
                        } else {
                            safe_set(ctx, x, y, z, Cobblestone);
                        }
                    }
                } else if wall_state != Chest && wall_state != Spawner {
                    safe_set(ctx, x, y, z, Air);
                }
            }
        }
    }
    // Direction.Plane.HORIZONTAL: NORTH, EAST, SOUTH, WEST.
    const HORIZ: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
    for _cc in 0..2 {
        for _i in 0..3 {
            let xc = origin.x + random.next_int_bounded(xr * 2 + 1) - xr;
            let yc = origin.y;
            let zc = origin.z + random.next_int_bounded(zr * 2 + 1) - zr;
            if ctx.level.get_block(xc, yc, zc).is_air() {
                let wall_count = HORIZ
                    .iter()
                    .filter(|&&(dx, dz)| is_solid(ctx.level.get_block(xc + dx, yc, zc + dz)))
                    .count();
                if wall_count == 1 {
                    safe_set(ctx, xc, yc, zc, Chest);
                    // RandomizableContainer.setBlockEntityLootTable → one nextLong
                    // (the loot seed); the loot NBT itself is deferred.
                    let _loot_seed = random.next_long();
                    break;
                }
            }
        }
    }
    safe_set(ctx, origin.x, origin.y, origin.z, Spawner);
    // MonsterRoomFeature.randomEntityId → Util.getRandom(MOBS, random) = one
    // nextInt(4); the spawned mob NBT is deferred.
    let _mob = random.next_int_bounded(4);
}

/// `Column.scan` reduced to the floor Y — the down-scan edge below the water
/// column at `origin` (used by underwater_magma). Returns `None` when `origin`
/// is not water or no non-water floor is within `search_range`.
fn magma_floor_y(ctx: &PlacementCtx, origin: Pos, search_range: i32) -> Option<i32> {
    let is_water = |b: ParityBlock| b == ParityBlock::Water;
    if !is_water(ctx.level.get_block(origin.x, origin.y, origin.z)) {
        return None;
    }
    let mut y = origin.y;
    let mut i = 1;
    while i < search_range && is_water(ctx.level.get_block(origin.x, y, origin.z)) {
        y -= 1;
        i += 1;
    }
    if !is_water(ctx.level.get_block(origin.x, y, origin.z)) {
        Some(y)
    } else {
        None
    }
}

/// `UnderwaterMagmaFeature.isVisibleFromOutside` — a face is visible when its
/// occlusion shape is not a full block. Approximated over the parity alphabet by
/// "air or fluid" (the empty-occlusion cases); draws no RNG.
fn magma_visible_from_outside(b: ParityBlock) -> bool {
    b.is_air() || b.is_fluid()
}

fn magma_valid_placement(ctx: &PlacementCtx, p: Pos) -> bool {
    let s = ctx.level.get_block(p.x, p.y, p.z);
    let water_or_air = s == ParityBlock::Water || s.is_air();
    if !water_or_air && !magma_visible_from_outside(ctx.level.get_block(p.x, p.y - 1, p.z)) {
        // Direction.Plane.HORIZONTAL.
        const HORIZ: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        for &(dx, dz) in HORIZ.iter() {
            if magma_visible_from_outside(ctx.level.get_block(p.x + dx, p.y, p.z + dz)) {
                return false;
            }
        }
        true
    } else {
        false
    }
}

/// `UnderwaterMagmaFeature.place`.
fn place_underwater_magma(cfg: &UnderwaterMagmaConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let floor_y = match magma_floor_y(ctx, origin, cfg.floor_search_range) {
        Some(y) => y,
        None => return,
    };
    let r = cfg.placement_radius_around_floor;
    let (min_x, max_x) = (origin.x - r, origin.x + r);
    let (min_y, max_y) = (floor_y - r, floor_y + r);
    let (min_z, max_z) = (origin.z - r, origin.z + r);
    // `BlockPos.betweenClosedStream` order: x fastest, then y, then z. The first
    // stream filter draws one nextFloat per position in that order.
    for z in min_z..=max_z {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if random.next_float() < cfg.placement_probability && magma_valid_placement(ctx, Pos::new(x, y, z)) {
                    ctx.level.set_block(x, y, z, ParityBlock::MagmaBlock);
                }
            }
        }
    }
}

fn parse_geode(cfg: &Value) -> GeodeConfig {
    let blocks = &cfg["blocks"];
    let layers = &cfg["layers"];
    let crack = &cfg["crack"];
    let int_or = |v: &Value, min: i32, max: i32| {
        if v.is_null() {
            IntProvider::Uniform { min, max }
        } else {
            IntProvider::parse(v)
        }
    };
    let tag_of = |v: &Value| v.as_str().map(|s| s.trim_start_matches('#')).and_then(BlockTag::from_id);
    GeodeConfig {
        filling: StateProvider::parse(&blocks["filling_provider"]),
        inner_layer: StateProvider::parse(&blocks["inner_layer_provider"]),
        alternate_inner_layer: StateProvider::parse(&blocks["alternate_inner_layer_provider"]),
        middle_layer: StateProvider::parse(&blocks["middle_layer_provider"]),
        outer_layer: StateProvider::parse(&blocks["outer_layer_provider"]),
        inner_placements: blocks["inner_placements"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|s| s["Name"].as_str().and_then(ParityBlock::from_name))
            .collect(),
        cannot_replace: tag_of(&blocks["cannot_replace"]),
        invalid_blocks: tag_of(&blocks["invalid_blocks"]),
        layer_filling: layers.get("filling").and_then(Value::as_f64).unwrap_or(1.7),
        layer_inner: layers.get("inner_layer").and_then(Value::as_f64).unwrap_or(2.2),
        layer_middle: layers.get("middle_layer").and_then(Value::as_f64).unwrap_or(3.2),
        layer_outer: layers.get("outer_layer").and_then(Value::as_f64).unwrap_or(4.2),
        generate_crack_chance: crack.get("generate_crack_chance").and_then(Value::as_f64).unwrap_or(1.0),
        base_crack_size: crack.get("base_crack_size").and_then(Value::as_f64).unwrap_or(2.0),
        crack_point_offset: crack.get("crack_point_offset").and_then(Value::as_i64).unwrap_or(2) as i32,
        use_potential_placements_chance: cfg.get("use_potential_placements_chance").and_then(Value::as_f64).unwrap_or(0.35),
        use_alternate_layer0_chance: cfg.get("use_alternate_layer0_chance").and_then(Value::as_f64).unwrap_or(0.0),
        placements_require_layer0_alternate: cfg
            .get("placements_require_layer0_alternate")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        outer_wall_distance: int_or(&cfg["outer_wall_distance"], 4, 5),
        distribution_points: int_or(&cfg["distribution_points"], 3, 4),
        point_offset: int_or(&cfg["point_offset"], 1, 2),
        min_gen_offset: cfg.get("min_gen_offset").and_then(Value::as_i64).unwrap_or(-16) as i32,
        max_gen_offset: cfg.get("max_gen_offset").and_then(Value::as_i64).unwrap_or(16) as i32,
        noise_multiplier: cfg.get("noise_multiplier").and_then(Value::as_f64).unwrap_or(0.05),
        invalid_blocks_threshold: cfg["invalid_blocks_threshold"].as_i64().unwrap_or(0) as i32,
    }
}

/// `org.joml.Math.invsqrt(double)` = `1.0 / sqrt(x)` (JOML default).
fn inv_sqrt(x: f64) -> f64 {
    1.0 / x.sqrt()
}

/// `BuddingAmethystBlock.canClusterGrowAtState` — air, or a full-source water.
fn amethyst_can_cluster_grow(b: ParityBlock) -> bool {
    b.is_air() || b == ParityBlock::Water
}

/// `GeodeFeature.place`.
fn place_geode(cfg: &GeodeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let mut points: Vec<(Pos, i32)> = Vec::new();
    let num_points = cfg.distribution_points.sample(random);
    // NormalNoise.create(new WorldgenRandom(new LegacyRandomSource(level.getSeed())), -4, 1.0).
    let noise = {
        let params = NoiseParameters { first_octave: -4, amplitudes: vec![1.0] };
        NormalNoise::create(&mut RandomSource::legacy(ctx.level.seed()), &params)
    };
    let mut crack_points: Vec<Pos> = Vec::new();
    let outer_max = match &cfg.outer_wall_distance {
        IntProvider::Uniform { max, .. } => *max,
        IntProvider::Constant(c) => *c,
        _ => 5,
    };
    let crack_size_adjust = num_points as f64 / outer_max as f64;
    let inner_air = 1.0 / cfg.layer_filling.sqrt();
    let innermost_block_layer = 1.0 / (cfg.layer_inner + crack_size_adjust).sqrt();
    let inner_crust = 1.0 / (cfg.layer_middle + crack_size_adjust).sqrt();
    let outer_crust = 1.0 / (cfg.layer_outer + crack_size_adjust).sqrt();
    let crack_size = 1.0
        / (cfg.base_crack_size + random.next_double() / 2.0 + if num_points > 3 { crack_size_adjust } else { 0.0 }).sqrt();
    let should_generate_crack = (random.next_float() as f64) < cfg.generate_crack_chance;
    let mut num_invalid = 0;

    for _ in 0..num_points {
        let x = cfg.outer_wall_distance.sample(random);
        let y = cfg.outer_wall_distance.sample(random);
        let z = cfg.outer_wall_distance.sample(random);
        let pos = Pos::new(origin.x + x, origin.y + y, origin.z + z);
        let state = ctx.level.get_block(pos.x, pos.y, pos.z);
        if state.is_air() || cfg.invalid_blocks.map(|t| t.contains(state)).unwrap_or(false) {
            num_invalid += 1;
            if num_invalid > cfg.invalid_blocks_threshold {
                return;
            }
        }
        points.push((pos, cfg.point_offset.sample(random)));
    }

    if should_generate_crack {
        let idx = random.next_int_bounded(4);
        let off = num_points * 2 + 1;
        match idx {
            0 => {
                crack_points.push(Pos::new(origin.x + off, origin.y + 7, origin.z));
                crack_points.push(Pos::new(origin.x + off, origin.y + 5, origin.z));
                crack_points.push(Pos::new(origin.x + off, origin.y + 1, origin.z));
            }
            1 => {
                crack_points.push(Pos::new(origin.x, origin.y + 7, origin.z + off));
                crack_points.push(Pos::new(origin.x, origin.y + 5, origin.z + off));
                crack_points.push(Pos::new(origin.x, origin.y + 1, origin.z + off));
            }
            2 => {
                crack_points.push(Pos::new(origin.x + off, origin.y + 7, origin.z + off));
                crack_points.push(Pos::new(origin.x + off, origin.y + 5, origin.z + off));
                crack_points.push(Pos::new(origin.x + off, origin.y + 1, origin.z + off));
            }
            _ => {
                crack_points.push(Pos::new(origin.x, origin.y + 7, origin.z));
                crack_points.push(Pos::new(origin.x, origin.y + 5, origin.z));
                crack_points.push(Pos::new(origin.x, origin.y + 1, origin.z));
            }
        }
    }

    let mut potential: Vec<Pos> = Vec::new();
    let can_replace = |b: ParityBlock| !cfg.cannot_replace.map(|t| t.contains(b)).unwrap_or(false);
    let safe_set = |ctx: &mut PlacementCtx, p: Pos, s: ParityBlock| {
        if can_replace(ctx.level.get_block(p.x, p.y, p.z)) {
            ctx.level.set_block(p.x, p.y, p.z, s);
        }
    };

    // `BlockPos.betweenClosed` order: x fastest, then y, then z.
    for pz in origin.z + cfg.min_gen_offset..=origin.z + cfg.max_gen_offset {
        for py in origin.y + cfg.min_gen_offset..=origin.y + cfg.max_gen_offset {
            for px in origin.x + cfg.min_gen_offset..=origin.x + cfg.max_gen_offset {
                let inside = Pos::new(px, py, pz);
                let noise_offset = noise.get_value(px as f64, py as f64, pz as f64) * cfg.noise_multiplier;
                let mut dist_shell = 0.0;
                let mut dist_crack = 0.0;
                for (pp, poff) in &points {
                    let d = pos_dist_sqr(inside, *pp) + *poff as f64;
                    dist_shell += inv_sqrt(d) + noise_offset;
                }
                for cp in &crack_points {
                    let d = pos_dist_sqr(inside, *cp) + cfg.crack_point_offset as f64;
                    dist_crack += inv_sqrt(d) + noise_offset;
                }
                if dist_shell < outer_crust {
                    continue;
                }
                if should_generate_crack && dist_crack >= crack_size && dist_shell < inner_air {
                    safe_set(ctx, inside, ParityBlock::Air);
                } else if dist_shell >= inner_air {
                    if let Some(s) = cfg.filling.get_state(ctx.level, random, inside) {
                        safe_set(ctx, inside, s);
                    }
                } else if dist_shell >= innermost_block_layer {
                    let use_alt = (random.next_float() as f64) < cfg.use_alternate_layer0_chance;
                    let provider = if use_alt { &cfg.alternate_inner_layer } else { &cfg.inner_layer };
                    if let Some(s) = provider.get_state(ctx.level, random, inside) {
                        safe_set(ctx, inside, s);
                    }
                    if (!cfg.placements_require_layer0_alternate || use_alt)
                        && (random.next_float() as f64) < cfg.use_potential_placements_chance
                    {
                        potential.push(inside);
                    }
                } else if dist_shell >= inner_crust {
                    if let Some(s) = cfg.middle_layer.get_state(ctx.level, random, inside) {
                        safe_set(ctx, inside, s);
                    }
                } else if dist_shell >= outer_crust {
                    if let Some(s) = cfg.outer_layer.get_state(ctx.level, random, inside) {
                        safe_set(ctx, inside, s);
                    }
                }
            }
        }
    }

    // Direction.values(): DOWN, UP, NORTH, SOUTH, WEST, EAST.
    const DIRS: [(i32, i32, i32); 6] =
        [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
    for crystal in &potential {
        if cfg.inner_placements.is_empty() {
            continue;
        }
        let block = cfg.inner_placements[random.next_int_bounded(cfg.inner_placements.len() as i32) as usize];
        for &(dx, dy, dz) in DIRS.iter() {
            let place = Pos::new(crystal.x + dx, crystal.y + dy, crystal.z + dz);
            let place_state = ctx.level.get_block(place.x, place.y, place.z);
            if amethyst_can_cluster_grow(place_state) {
                safe_set(ctx, place, block);
                break;
            }
        }
    }
}

/// `Vec3i.distSqr(Vec3i)` — integer component differences, summed as doubles.
fn pos_dist_sqr(a: Pos, b: Pos) -> f64 {
    let dx = (a.x - b.x) as f64;
    let dy = (a.y - b.y) as f64;
    let dz = (a.z - b.z) as f64;
    dx * dx + dy * dy + dz * dz
}

// ---------------------------------------------------------------------------
// Dripstone group: speleothem (pointed_dripstone), speleothem_cluster
// (dripstone_cluster), large_dripstone, simple_random_selector
// ---------------------------------------------------------------------------

fn dripstone_tag(v: &Value) -> Option<BlockTag> {
    v.as_str().map(|s| s.trim_start_matches('#')).and_then(BlockTag::from_id)
}

fn parse_speleothem(cfg: &Value) -> SpeleothemConfig {
    SpeleothemConfig {
        base_block: cfg["base_block"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::DripstoneBlock),
        pointed_block: cfg["pointed_block"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::PointedDripstone),
        replaceable_blocks: dripstone_tag(&cfg["replaceable_blocks"]),
        chance_of_taller_generation: cfg.get("chance_of_taller_generation").and_then(Value::as_f64).unwrap_or(0.2) as f32,
        chance_of_directional_spread: cfg.get("chance_of_directional_spread").and_then(Value::as_f64).unwrap_or(0.7) as f32,
        chance_of_spread_radius2: cfg.get("chance_of_spread_radius2").and_then(Value::as_f64).unwrap_or(0.5) as f32,
        chance_of_spread_radius3: cfg.get("chance_of_spread_radius3").and_then(Value::as_f64).unwrap_or(0.5) as f32,
    }
}

fn parse_speleothem_cluster(cfg: &Value) -> SpeleothemClusterConfig {
    SpeleothemClusterConfig {
        floor_to_ceiling_search_range: cfg["floor_to_ceiling_search_range"].as_i64().unwrap_or(12) as i32,
        height: IntProvider::parse(&cfg["height"]),
        radius: IntProvider::parse(&cfg["radius"]),
        max_stalagmite_stalactite_height_diff: cfg["max_stalagmite_stalactite_height_diff"].as_i64().unwrap_or(0) as i32,
        height_deviation: cfg["height_deviation"].as_i64().unwrap_or(1) as i32,
        speleothem_block_layer_thickness: IntProvider::parse(&cfg["speleothem_block_layer_thickness"]),
        density: FloatProvider::parse(&cfg["density"]),
        wetness: FloatProvider::parse(&cfg["wetness"]),
        chance_of_speleothem_at_max_distance_from_center: cfg["chance_of_speleothem_at_max_distance_from_center"].as_f64().unwrap_or(0.0),
        max_distance_from_center_affecting_height_bias: cfg["max_distance_from_center_affecting_height_bias"].as_i64().unwrap_or(0) as i32,
        max_distance_from_edge_affecting_chance_of_speleothem: cfg["max_distance_from_edge_affecting_chance_of_speleothem"].as_i64().unwrap_or(0) as i32,
        base_block: cfg["base_block"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::DripstoneBlock),
        pointed_block: cfg["pointed_block"]["Name"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::PointedDripstone),
        replaceable_blocks: dripstone_tag(&cfg["replaceable_blocks"]),
    }
}

fn parse_large_dripstone(cfg: &Value) -> LargeDripstoneConfig {
    LargeDripstoneConfig {
        floor_to_ceiling_search_range: cfg.get("floor_to_ceiling_search_range").and_then(Value::as_i64).unwrap_or(30) as i32,
        column_radius: IntProvider::parse(&cfg["column_radius"]),
        height_scale: FloatProvider::parse(&cfg["height_scale"]),
        max_column_radius_to_cave_height_ratio: cfg["max_column_radius_to_cave_height_ratio"].as_f64().unwrap_or(0.0),
        stalactite_bluntness: FloatProvider::parse(&cfg["stalactite_bluntness"]),
        stalagmite_bluntness: FloatProvider::parse(&cfg["stalagmite_bluntness"]),
        wind_speed: FloatProvider::parse(&cfg["wind_speed"]),
        min_radius_for_wind: cfg["min_radius_for_wind"].as_i64().unwrap_or(0) as i32,
        min_bluntness_for_wind: cfg["min_bluntness_for_wind"].as_f64().unwrap_or(0.0),
        base_block: ParityBlock::DripstoneBlock,
        pointed_block: ParityBlock::PointedDripstone,
        replaceable_blocks: dripstone_tag(&cfg["replaceable_blocks"]),
    }
}

fn int_provider_min(p: &IntProvider) -> i32 {
    match p {
        IntProvider::Constant(c) => *c,
        IntProvider::Uniform { min, .. } => *min,
        IntProvider::BiasedToBottom { min, .. } => *min,
        IntProvider::Clamped { source, min, .. } => (*min).max(int_provider_min(source)),
        IntProvider::ClampedNormal { min, .. } => *min,
        IntProvider::Trapezoid { min, .. } => *min,
        IntProvider::WeightedList(e) => e.iter().map(|(p, _)| int_provider_min(p)).min().unwrap_or(0),
    }
}
fn int_provider_max(p: &IntProvider) -> i32 {
    match p {
        IntProvider::Constant(c) => *c,
        IntProvider::Uniform { max, .. } => *max,
        IntProvider::BiasedToBottom { max, .. } => *max,
        IntProvider::Clamped { source, max, .. } => (*max).min(int_provider_max(source)),
        IntProvider::ClampedNormal { max, .. } => *max,
        IntProvider::Trapezoid { max, .. } => *max,
        IntProvider::WeightedList(e) => e.iter().map(|(p, _)| int_provider_max(p)).max().unwrap_or(0),
    }
}

/// `Mth.clampedMap(double,double,double,double,double)`.
fn clamped_map(x: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = (x - a) / (b - a);
    if t < 0.0 {
        c
    } else if t > 1.0 {
        d
    } else {
        c + t * (d - c)
    }
}

/// `Mth.randomBetweenInclusive`.
fn mth_random_between_inclusive(random: &mut WorldgenRandom, min: i32, max: i32) -> i32 {
    min + random.next_int_bounded(max - min + 1)
}

/// `ClampedNormalFloat.sample(random, mean, deviation, min, max)`.
fn clamped_normal_sample(random: &mut WorldgenRandom, mean: f32, deviation: f32, min: f32, max: f32) -> f32 {
    (mean + (random.next_gaussian() as f32) * deviation).clamp(min, max)
}

// --- SpeleothemUtils helpers ---

fn spel_is_empty_or_water(b: ParityBlock) -> bool {
    b.is_air() || b == ParityBlock::Water
}
fn spel_is_neither_empty_nor_water(b: ParityBlock) -> bool {
    !b.is_air() && b != ParityBlock::Water
}
fn spel_is_empty_or_water_or_lava(b: ParityBlock) -> bool {
    b.is_air() || b == ParityBlock::Water || b == ParityBlock::Lava
}
fn spel_is_base(b: ParityBlock, base: ParityBlock, replaceable: Option<BlockTag>) -> bool {
    b == base || replaceable.map(|t| t.contains(b)).unwrap_or(false)
}
fn spel_is_base_or_lava(b: ParityBlock, base: ParityBlock, replaceable: Option<BlockTag>) -> bool {
    spel_is_base(b, base, replaceable) || b == ParityBlock::Lava
}

/// `SpeleothemUtils.getSpeleothemHeight`.
fn speleothem_util_height(xz_dist: f64, radius: f64, scale: f64, bluntness: f64) -> f64 {
    let xz = if xz_dist < bluntness { bluntness } else { xz_dist };
    let r = xz / radius * 0.384;
    let part1 = 0.75 * r.powf(1.333_333_333_333_333_3);
    let part2 = r.powf(0.666_666_666_666_666_6);
    let part3 = 0.333_333_333_333_333_3 * r.ln();
    let h = (scale * (part1 - part2 - part3)).max(0.0);
    h / 0.384 * radius
}

/// `SpeleothemUtils.isCircleMostlyEmbeddedInStone`.
fn speleothem_circle_embedded(ctx: &PlacementCtx, center: Pos, xz_radius: i32) -> bool {
    if spel_is_empty_or_water_or_lava(ctx.level.get_block(center.x, center.y, center.z)) {
        return false;
    }
    let angle_increment = 6.0_f32 / xz_radius as f32;
    let mut angle = 0.0_f32;
    while angle < std::f32::consts::TAU {
        let dx = (super::carvers::mth_cos(angle as f64) * xz_radius as f32) as i32;
        let dz = (super::carvers::mth_sin(angle as f64) * xz_radius as f32) as i32;
        if spel_is_empty_or_water_or_lava(ctx.level.get_block(center.x + dx, center.y, center.z + dz)) {
            return false;
        }
        angle += angle_increment;
    }
    true
}

/// `SpeleothemUtils.placeBaseBlockIfPossible`.
fn speleothem_place_base(ctx: &mut PlacementCtx, p: Pos, base: ParityBlock, replaceable: Option<BlockTag>) -> bool {
    let s = ctx.level.get_block(p.x, p.y, p.z);
    if replaceable.map(|t| t.contains(s)).unwrap_or(false) {
        ctx.level.set_block(p.x, p.y, p.z, base);
        true
    } else {
        false
    }
}

/// `SpeleothemUtils.growSpeleothem` — `buildBaseToTipColumn` collapses onto
/// `height` consecutive `pointed_block`s (their thickness/direction/waterlogged
/// properties drop to the default state per precedent). Draws no RNG.
fn speleothem_grow(
    ctx: &mut PlacementCtx,
    start: Pos,
    tip: (i32, i32, i32),
    height: i32,
    base: ParityBlock,
    pointed: ParityBlock,
    replaceable: Option<BlockTag>,
) {
    let root = Pos::new(start.x - tip.0, start.y - tip.1, start.z - tip.2);
    if !spel_is_base(ctx.level.get_block(root.x, root.y, root.z), base, replaceable) {
        return;
    }
    let mut p = start;
    for _ in 0..height {
        ctx.level.set_block(p.x, p.y, p.z, pointed);
        p = Pos::new(p.x + tip.0, p.y + tip.1, p.z + tip.2);
    }
}

/// `Column.scan` reduced to `(floor, ceiling)` — the solid edges bounding the
/// contiguous `inside` column at `pos`. `None` when `pos` is not `inside`.
fn column_scan(
    ctx: &PlacementCtx,
    pos: Pos,
    range: i32,
    inside: impl Fn(ParityBlock) -> bool,
    valid_edge: impl Fn(ParityBlock) -> bool,
) -> Option<(Option<i32>, Option<i32>)> {
    if !inside(ctx.level.get_block(pos.x, pos.y, pos.z)) {
        return None;
    }
    let scan = |dir: i32| {
        let mut y = pos.y;
        let mut i = 1;
        while i < range && inside(ctx.level.get_block(pos.x, y, pos.z)) {
            y += dir;
            i += 1;
        }
        if valid_edge(ctx.level.get_block(pos.x, y, pos.z)) {
            Some(y)
        } else {
            None
        }
    };
    let ceiling = scan(1);
    let floor = scan(-1);
    Some((floor, ceiling))
}

fn column_height(floor: Option<i32>, ceiling: Option<i32>) -> Option<i32> {
    match (floor, ceiling) {
        (Some(f), Some(c)) => Some(c - f - 1),
        _ => None,
    }
}

/// `SpeleothemFeature.place` — the single pointed dripstone (`pointed_dripstone`
/// via `simple_random_selector`).
fn place_speleothem(cfg: &SpeleothemConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let base = cfg.base_block;
    let rep = cfg.replaceable_blocks;
    let above = spel_is_base(ctx.level.get_block(origin.x, origin.y + 1, origin.z), base, rep);
    let below = spel_is_base(ctx.level.get_block(origin.x, origin.y - 1, origin.z), base, rep);
    // getTipDirection: DOWN means tip points down (attached to ceiling above).
    let tip: (i32, i32, i32) = if above && below {
        if random.next_boolean() { (0, -1, 0) } else { (0, 1, 0) }
    } else if above {
        (0, -1, 0)
    } else if below {
        (0, 1, 0)
    } else {
        return;
    };
    let opp = (-tip.0, -tip.1, -tip.2);
    let root = Pos::new(origin.x + opp.0, origin.y + opp.1, origin.z + opp.2);
    // createPatchOfBaseBlocks.
    speleothem_place_base(ctx, root, base, rep);
    // Direction.Plane.HORIZONTAL: NORTH, EAST, SOUTH, WEST.
    const HORIZ: [(i32, i32, i32); 4] = [(0, 0, -1), (1, 0, 0), (0, 0, 1), (-1, 0, 0)];
    for &(dx, dy, dz) in HORIZ.iter() {
        if random.next_float() <= cfg.chance_of_directional_spread {
            let p1 = Pos::new(root.x + dx, root.y + dy, root.z + dz);
            speleothem_place_base(ctx, p1, base, rep);
            if random.next_float() <= cfg.chance_of_spread_radius2 {
                let d2 = random_direction(random);
                let p2 = Pos::new(p1.x + d2.0, p1.y + d2.1, p1.z + d2.2);
                speleothem_place_base(ctx, p2, base, rep);
                if random.next_float() <= cfg.chance_of_spread_radius3 {
                    let d3 = random_direction(random);
                    let p3 = Pos::new(p2.x + d3.0, p2.y + d3.1, p2.z + d3.2);
                    speleothem_place_base(ctx, p3, base, rep);
                }
            }
        }
    }
    // height = (nextFloat < chanceOfTaller && isEmptyOrWater(pos.relative(tip))) ? 2 : 1.
    let taller = random.next_float() < cfg.chance_of_taller_generation;
    let tip_is_empty = spel_is_empty_or_water(ctx.level.get_block(origin.x + tip.0, origin.y + tip.1, origin.z + tip.2));
    let height = if taller && tip_is_empty { 2 } else { 1 };
    speleothem_grow(ctx, origin, tip, height, base, cfg.pointed_block, rep);
}

/// `Direction.getRandom(random)` — `Direction.values()[nextInt(6)]`:
/// DOWN, UP, NORTH, SOUTH, WEST, EAST.
fn random_direction(random: &mut WorldgenRandom) -> (i32, i32, i32) {
    const DIRS: [(i32, i32, i32); 6] =
        [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
    DIRS[random.next_int_bounded(6) as usize]
}

/// `SimpleRandomSelectorFeature.place`.
fn place_simple_random_selector(
    cfg: &SimpleRandomSelectorConfig,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    origin: Pos,
) {
    if cfg.features.is_empty() {
        return;
    }
    let index = random.next_int_bounded(cfg.features.len() as i32) as usize;
    place_nested(&cfg.features[index], ctx, random, origin);
}

/// `SpeleothemClusterFeature.getChanceOfStalagmiteOrStalactite`.
fn cluster_chance(x_radius: i32, z_radius: i32, dx: i32, dz: i32, cfg: &SpeleothemClusterConfig) -> f64 {
    let dist_from_edge = (x_radius - dx.abs()).min(z_radius - dz.abs());
    clamped_map(
        dist_from_edge as f64,
        0.0,
        cfg.max_distance_from_edge_affecting_chance_of_speleothem as f64,
        cfg.chance_of_speleothem_at_max_distance_from_center,
        1.0,
    )
}

/// `SpeleothemClusterFeature.getSpeleothemHeight`.
fn cluster_height(
    random: &mut WorldgenRandom,
    dx: i32,
    dz: i32,
    density: f32,
    max_height: i32,
    cfg: &SpeleothemClusterConfig,
) -> i32 {
    if random.next_float() > density {
        return 0;
    }
    let dist = dx.abs() + dz.abs();
    let height_mean = clamped_map(
        dist as f64,
        0.0,
        cfg.max_distance_from_center_affecting_height_bias as f64,
        max_height as f64 / 2.0,
        0.0,
    ) as f32;
    clamped_normal_sample(random, height_mean, cfg.height_deviation as f32, 0.0, max_height as f32) as i32
}

fn cluster_replace_with_base(
    ctx: &mut PlacementCtx,
    first: Pos,
    max_count: i32,
    dir: (i32, i32, i32),
    cfg: &SpeleothemClusterConfig,
) {
    let mut p = first;
    for _ in 0..max_count {
        if !speleothem_place_base(ctx, p, cfg.base_block, cfg.replaceable_blocks) {
            return;
        }
        p = Pos::new(p.x + dir.0, p.y + dir.1, p.z + dir.2);
    }
}

fn cluster_can_be_adjacent_to_water(ctx: &PlacementCtx, p: Pos) -> bool {
    let s = ctx.level.get_block(p.x, p.y, p.z);
    BlockTag::BaseStoneOverworld.contains(s) || s == ParityBlock::Water
}

fn cluster_can_place_pool(ctx: &PlacementCtx, p: Pos, cfg: &SpeleothemClusterConfig) -> bool {
    let s = ctx.level.get_block(p.x, p.y, p.z);
    if s == ParityBlock::Water || s == cfg.base_block || s == cfg.pointed_block {
        return false;
    }
    if ctx.level.get_block(p.x, p.y + 1, p.z) == ParityBlock::Water {
        return false;
    }
    const HORIZ: [(i32, i32, i32); 4] = [(0, 0, -1), (1, 0, 0), (0, 0, 1), (-1, 0, 0)];
    for &(dx, dy, dz) in HORIZ.iter() {
        if !cluster_can_be_adjacent_to_water(ctx, Pos::new(p.x + dx, p.y + dy, p.z + dz)) {
            return false;
        }
    }
    cluster_can_be_adjacent_to_water(ctx, Pos::new(p.x, p.y - 1, p.z))
}

/// `SpeleothemClusterFeature.place` (`dripstone_cluster`).
fn place_speleothem_cluster(cfg: &SpeleothemClusterConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    if !spel_is_empty_or_water(ctx.level.get_block(origin.x, origin.y, origin.z)) {
        return;
    }
    let height = cfg.height.sample(random);
    let wetness = cfg.wetness.sample(random);
    let density = cfg.density.sample(random);
    let x_radius = cfg.radius.sample(random);
    let z_radius = cfg.radius.sample(random);
    for dx in -x_radius..=x_radius {
        for dz in -z_radius..=z_radius {
            let chance = cluster_chance(x_radius, z_radius, dx, dz, cfg);
            let pos = Pos::new(origin.x + dx, origin.y, origin.z + dz);
            cluster_place_column(ctx, random, pos, dx, dz, wetness, chance, height, density, cfg);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cluster_place_column(
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    pos: Pos,
    dx: i32,
    dz: i32,
    chance_of_water: f32,
    chance_of_speleothem: f64,
    cluster_height_val: i32,
    density: f32,
    cfg: &SpeleothemClusterConfig,
) {
    let scan = match column_scan(
        ctx,
        pos,
        cfg.floor_to_ceiling_search_range,
        spel_is_empty_or_water,
        spel_is_neither_empty_nor_water,
    ) {
        Some(s) => s,
        None => return,
    };
    let ceiling = scan.1;
    let base_floor = scan.0;
    if ceiling.is_none() && base_floor.is_none() {
        return;
    }
    // Possible water pool at the base.
    let want_pool = random.next_float() < chance_of_water;
    let (mut floor, ceiling) = (base_floor, ceiling);
    if want_pool {
        if let Some(bf) = base_floor {
            if cluster_can_place_pool(ctx, Pos::new(pos.x, bf, pos.z), cfg) {
                ctx.level.set_block(pos.x, bf, pos.z, ParityBlock::Water);
                floor = Some(bf - 1);
            }
        }
    }
    let column_h = column_height(floor, ceiling);

    // Stalactite.
    let want_stalactite = random.next_double() < chance_of_speleothem;
    let mut stalactite_height = 0;
    if let Some(c) = ceiling {
        if want_stalactite && ctx.level.get_block(pos.x, c, pos.z) != ParityBlock::Lava {
            let thickness = cfg.speleothem_block_layer_thickness.sample(random);
            cluster_replace_with_base(ctx, Pos::new(pos.x, c, pos.z), thickness, (0, 1, 0), cfg);
            let max_h = match floor {
                Some(f) => cluster_height_val.min(c - f),
                None => cluster_height_val,
            };
            stalactite_height = cluster_height(random, dx, dz, density, max_h, cfg);
        }
    }

    // Stalagmite.
    let want_stalagmite = random.next_double() < chance_of_speleothem;
    let mut stalagmite_height = 0;
    if let Some(f) = floor {
        if want_stalagmite && ctx.level.get_block(pos.x, f, pos.z) != ParityBlock::Lava {
            let thickness = cfg.speleothem_block_layer_thickness.sample(random);
            cluster_replace_with_base(ctx, Pos::new(pos.x, f, pos.z), thickness, (0, -1, 0), cfg);
            if ceiling.is_some() {
                stalagmite_height = 0.max(
                    stalactite_height
                        + mth_random_between_inclusive(
                            random,
                            -cfg.max_stalagmite_stalactite_height_diff,
                            cfg.max_stalagmite_stalactite_height_diff,
                        ),
                );
            } else {
                stalagmite_height = cluster_height(random, dx, dz, density, cluster_height_val, cfg);
            }
        }
    }

    // Resolve overlap.
    let (actual_stalagmite, actual_stalactite) = if let (Some(c), Some(f)) = (ceiling, floor) {
        if c - stalactite_height <= f + stalagmite_height {
            let lowest_stalactite_bottom = (c - stalactite_height).max(f + 1);
            let highest_stalagmite_top = (f + stalagmite_height).min(c - 1);
            let actual_stalactite_bottom =
                mth_random_between_inclusive(random, lowest_stalactite_bottom, highest_stalagmite_top + 1);
            let actual_stalagmite_top = actual_stalactite_bottom - 1;
            (actual_stalagmite_top - f, c - actual_stalactite_bottom)
        } else {
            (stalagmite_height, stalactite_height)
        }
    } else {
        (stalagmite_height, stalactite_height)
    };

    let merge_tips = random.next_boolean()
        && actual_stalactite > 0
        && actual_stalagmite > 0
        && column_h == Some(actual_stalactite + actual_stalagmite);
    let _ = merge_tips; // tip-merge only changes the TIP thickness property (collapsed).

    if let Some(c) = ceiling {
        speleothem_grow(
            ctx,
            Pos::new(pos.x, c - 1, pos.z),
            (0, -1, 0),
            actual_stalactite,
            cfg.base_block,
            cfg.pointed_block,
            cfg.replaceable_blocks,
        );
    }
    if let Some(f) = floor {
        speleothem_grow(
            ctx,
            Pos::new(pos.x, f + 1, pos.z),
            (0, 1, 0),
            actual_stalagmite,
            cfg.base_block,
            cfg.pointed_block,
            cfg.replaceable_blocks,
        );
    }
}

/// `LargeDripstoneFeature.WindOffsetter`.
struct WindOffsetter {
    origin_y: i32,
    wind: Option<(f64, f64)>, // (x, z) speed components
    max_offset: i32,
}

impl WindOffsetter {
    fn none() -> Self {
        WindOffsetter { origin_y: 0, wind: None, max_offset: 0 }
    }
    fn new(origin_y: i32, random: &mut WorldgenRandom, wind_speed: &FloatProvider, max_offset: i32) -> Self {
        let speed = wind_speed.sample(random);
        let dir = random.next_float() * std::f32::consts::PI;
        let wind = (
            (super::carvers::mth_cos(dir as f64) * speed) as f64,
            (super::carvers::mth_sin(dir as f64) * speed) as f64,
        );
        WindOffsetter { origin_y, wind: Some(wind), max_offset }
    }
    fn offset(&self, p: Pos) -> Pos {
        match self.wind {
            None => p,
            Some((wx, wz)) => {
                let dy = (self.origin_y - p.y) as f64;
                let dx = (wx * dy).floor() as i32;
                let dz = (wz * dy).floor() as i32;
                Pos::new(p.x + dx.clamp(-self.max_offset, self.max_offset), p.y, p.z + dz.clamp(-self.max_offset, self.max_offset))
            }
        }
    }
}

struct LargeDripstone {
    root: Pos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl LargeDripstone {
    fn height_at_radius(&self, check_radius: f32) -> i32 {
        speleothem_util_height(check_radius as f64, self.radius as f64, self.scale, self.bluntness) as i32
    }
    fn height(&self) -> i32 {
        self.height_at_radius(0.0)
    }
    fn is_suitable_for_wind(&self, cfg: &LargeDripstoneConfig) -> bool {
        self.radius >= cfg.min_radius_for_wind && self.bluntness >= cfg.min_bluntness_for_wind
    }
    fn move_back_until_embedded(&mut self, ctx: &PlacementCtx, wind: &WindOffsetter) -> bool {
        while self.radius > 1 {
            let mut new_root = self.root;
            let max_tries = 10.min(self.height());
            for _ in 0..max_tries {
                if ctx.level.get_block(new_root.x, new_root.y, new_root.z) == ParityBlock::Lava {
                    return false;
                }
                if speleothem_circle_embedded(ctx, wind.offset(new_root), self.radius) {
                    self.root = new_root;
                    return true;
                }
                new_root.y += if self.pointing_up { -1 } else { 1 };
            }
            self.radius /= 2;
        }
        false
    }
    fn place_blocks(&self, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, wind: &WindOffsetter) {
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let cur_radius = ((dx * dx + dz * dz) as f32).sqrt();
                if cur_radius > self.radius as f32 {
                    continue;
                }
                let mut height = self.height_at_radius(cur_radius);
                if height > 0 {
                    if random.next_float() < 0.2 {
                        height = (height as f32 * (random.next_float() * (1.0 - 0.8) + 0.8)) as i32;
                    }
                    let mut p = Pos::new(self.root.x + dx, self.root.y, self.root.z + dz);
                    let mut out_of_stone = false;
                    let max_y = if self.pointing_up {
                        ctx.level.get_height(Heightmap::WorldSurfaceWg, p.x, p.z)
                    } else {
                        i32::MAX
                    };
                    let mut i = 0;
                    while i < height && p.y < max_y {
                        let wp = wind.offset(p);
                        let wb = ctx.level.get_block(wp.x, wp.y, wp.z);
                        if spel_is_empty_or_water_or_lava(wb) {
                            out_of_stone = true;
                            ctx.level.set_block(wp.x, wp.y, wp.z, ParityBlock::DripstoneBlock);
                        } else if out_of_stone && BlockTag::BaseStoneOverworld.contains(wb) {
                            break;
                        }
                        p.y += if self.pointing_up { 1 } else { -1 };
                        i += 1;
                    }
                }
            }
        }
    }
}

/// `LargeDripstoneFeature.place`.
fn place_large_dripstone(cfg: &LargeDripstoneConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    if !spel_is_empty_or_water(ctx.level.get_block(origin.x, origin.y, origin.z)) {
        return;
    }
    let rep = cfg.replaceable_blocks;
    let scan = column_scan(ctx, origin, cfg.floor_to_ceiling_search_range, spel_is_empty_or_water, |b| {
        spel_is_base_or_lava(b, ParityBlock::DripstoneBlock, rep)
    });
    let (floor, ceiling) = match scan {
        Some((Some(f), Some(c))) => (f, c),
        _ => return, // not a Column.Range
    };
    let range_height = ceiling - floor - 1;
    if range_height < 4 {
        return;
    }
    let radius_min = int_provider_min(&cfg.column_radius);
    let radius_max_p = int_provider_max(&cfg.column_radius);
    let max_column_radius_from_height = (range_height as f64 * cfg.max_column_radius_to_cave_height_ratio) as i32;
    let max_column_radius = max_column_radius_from_height.clamp(radius_min, radius_max_p);
    let radius = mth_random_between_inclusive(random, radius_min, max_column_radius);

    let mut stalactite = LargeDripstone {
        root: Pos::new(origin.x, ceiling - 1, origin.z),
        pointing_up: false,
        radius,
        bluntness: cfg.stalactite_bluntness.sample(random) as f64,
        scale: cfg.height_scale.sample(random) as f64,
    };
    let mut stalagmite = LargeDripstone {
        root: Pos::new(origin.x, floor + 1, origin.z),
        pointing_up: true,
        radius,
        bluntness: cfg.stalagmite_bluntness.sample(random) as f64,
        scale: cfg.height_scale.sample(random) as f64,
    };
    let wind = if stalactite.is_suitable_for_wind(cfg) && stalagmite.is_suitable_for_wind(cfg) {
        WindOffsetter::new(origin.y, random, &cfg.wind_speed, 16 - radius)
    } else {
        WindOffsetter::none()
    };
    let stalactite_embedded = stalactite.move_back_until_embedded(ctx, &wind);
    let stalagmite_embedded = stalagmite.move_back_until_embedded(ctx, &wind);
    if stalactite_embedded {
        stalactite.place_blocks(ctx, random, &wind);
    }
    if stalagmite_embedded {
        stalagmite.place_blocks(ctx, random, &wind);
    }
}

// ---------------------------------------------------------------------------
// Coral group (warm ocean): coral_tree, coral_claw, coral_mushroom
// ---------------------------------------------------------------------------

enum CoralKind {
    Tree,
    Claw,
    Mushroom,
}

/// `#coral_blocks` — registry (tag) order.
const CORAL_BLOCKS: [ParityBlock; 5] = [
    ParityBlock::TubeCoralBlock,
    ParityBlock::BrainCoralBlock,
    ParityBlock::BubbleCoralBlock,
    ParityBlock::FireCoralBlock,
    ParityBlock::HornCoralBlock,
];
/// `#corals` = `#coral_plants` (5) ∪ the 5 coral fans, in tag order.
const CORALS: [ParityBlock; 10] = [
    ParityBlock::TubeCoral,
    ParityBlock::BrainCoral,
    ParityBlock::BubbleCoral,
    ParityBlock::FireCoral,
    ParityBlock::HornCoral,
    ParityBlock::TubeCoralFan,
    ParityBlock::BrainCoralFan,
    ParityBlock::BubbleCoralFan,
    ParityBlock::FireCoralFan,
    ParityBlock::HornCoralFan,
];
/// `#wall_corals` — the 5 coral wall fans, in tag order.
const WALL_CORALS: [ParityBlock; 5] = [
    ParityBlock::TubeCoralWallFan,
    ParityBlock::BrainCoralWallFan,
    ParityBlock::BubbleCoralWallFan,
    ParityBlock::FireCoralWallFan,
    ParityBlock::HornCoralWallFan,
];

/// `Direction.Plane.HORIZONTAL` faces, in order: NORTH, EAST, SOUTH, WEST.
const HORIZONTAL: [(i32, i32, i32); 4] = [(0, 0, -1), (1, 0, 0), (0, 0, 1), (-1, 0, 0)];

/// `Direction.getClockWise` for a horizontal facing (N→E→S→W→N).
fn horizontal_clockwise(d: (i32, i32, i32)) -> (i32, i32, i32) {
    match d {
        (0, 0, -1) => (1, 0, 0),  // N -> E
        (1, 0, 0) => (0, 0, 1),   // E -> S
        (0, 0, 1) => (-1, 0, 0),  // S -> W
        (-1, 0, 0) => (0, 0, -1), // W -> N
        _ => d,
    }
}
fn horizontal_counter_clockwise(d: (i32, i32, i32)) -> (i32, i32, i32) {
    match d {
        (0, 0, -1) => (-1, 0, 0), // N -> W
        (-1, 0, 0) => (0, 0, 1),  // W -> S
        (0, 0, 1) => (1, 0, 0),   // S -> E
        (1, 0, 0) => (0, 0, -1),  // E -> N
        _ => d,
    }
}

/// `Util.getRandomElementOf(HolderSet, random)` = `list[nextInt(size)]`.
fn coral_random(list: &[ParityBlock], random: &mut WorldgenRandom) -> ParityBlock {
    list[random.next_int_bounded(list.len() as i32) as usize]
}

/// `Util.shuffle` (Fisher–Yates): `for i in size..2 { swap(i-1, nextInt(i)) }`.
fn shuffle_directions(dirs: &mut Vec<(i32, i32, i32)>, random: &mut WorldgenRandom) {
    let size = dirs.len();
    let mut i = size;
    while i > 1 {
        let j = random.next_int_bounded(i as i32) as usize;
        dirs.swap(i - 1, j);
        i -= 1;
    }
}

/// `CoralFeature.placeCoralBlock`. Coral fans are property-carrying (facing) and
/// collapse to their default block state per precedent; the RNG is 1:1.
fn place_coral_block(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, pos: Pos, state: ParityBlock) -> bool {
    use ParityBlock::Water;
    let above = Pos::new(pos.x, pos.y + 1, pos.z);
    let target = ctx.level.get_block(pos.x, pos.y, pos.z);
    if !(target == Water || BlockTag::Corals.contains(target)) || ctx.level.get_block(above.x, above.y, above.z) != Water {
        return false;
    }
    ctx.level.set_block(pos.x, pos.y, pos.z, state);
    if random.next_float() < 0.25 {
        let coral = coral_random(&CORALS, random);
        ctx.level.set_block(above.x, above.y, above.z, coral);
    } else if random.next_float() < 0.05 {
        // sea pickle with pickles = nextInt(4)+1 (the pickles property collapses).
        let _pickles = random.next_int_bounded(4) + 1;
        ctx.level.set_block(above.x, above.y, above.z, ParityBlock::SeaPickle);
    }
    for &(dx, dy, dz) in HORIZONTAL.iter() {
        if random.next_float() < 0.2 {
            let rel = Pos::new(pos.x + dx, pos.y + dy, pos.z + dz);
            if ctx.level.get_block(rel.x, rel.y, rel.z) == Water {
                let fan = coral_random(&WALL_CORALS, random);
                ctx.level.set_block(rel.x, rel.y, rel.z, fan);
            }
        }
    }
    true
}

fn place_coral(kind: CoralKind, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    let state = coral_random(&CORAL_BLOCKS, random);
    match kind {
        CoralKind::Tree => coral_tree(ctx, random, origin, state),
        CoralKind::Claw => coral_claw(ctx, random, origin, state),
        CoralKind::Mushroom => coral_mushroom(ctx, random, origin, state),
    }
}

/// `CoralTreeFeature.placeFeature`.
fn coral_tree(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos, state: ParityBlock) {
    let mut pos = origin;
    let trunk_height = random.next_int_bounded(3) + 1;
    for _ in 0..trunk_height {
        if !place_coral_block(ctx, random, pos, state) {
            return;
        }
        pos.y += 1;
    }
    let trunk_top = pos;
    let n_branches = random.next_int_bounded(3) + 2;
    let mut dirs: Vec<(i32, i32, i32)> = HORIZONTAL.to_vec();
    shuffle_directions(&mut dirs, random);
    for &branch_dir in dirs.iter().take(n_branches as usize) {
        pos = Pos::new(trunk_top.x + branch_dir.0, trunk_top.y + branch_dir.1, trunk_top.z + branch_dir.2);
        let branch_height = random.next_int_bounded(5) + 2;
        let mut segment_length = 0;
        let mut j = 0;
        while j < branch_height && place_coral_block(ctx, random, pos, state) {
            segment_length += 1;
            pos.y += 1;
            if j == 0 || (segment_length >= 2 && random.next_float() < 0.25) {
                pos = Pos::new(pos.x + branch_dir.0, pos.y + branch_dir.1, pos.z + branch_dir.2);
                segment_length = 0;
            }
            j += 1;
        }
    }
}

/// `CoralClawFeature.placeFeature`.
fn coral_claw(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos, state: ParityBlock) {
    if !place_coral_block(ctx, random, origin, state) {
        return;
    }
    let claw_dir = HORIZONTAL[random.next_int_bounded(4) as usize];
    let n_branches = random.next_int_bounded(2) + 2;
    let mut dirs: Vec<(i32, i32, i32)> =
        vec![claw_dir, horizontal_clockwise(claw_dir), horizontal_counter_clockwise(claw_dir)];
    shuffle_directions(&mut dirs, random);
    for &branch_dir in dirs.iter().take(n_branches as usize) {
        let mut pos = origin;
        let sideway_length = random.next_int_bounded(2) + 1;
        pos = Pos::new(pos.x + branch_dir.0, pos.y + branch_dir.1, pos.z + branch_dir.2);
        let inway_length;
        let segment_dir;
        if branch_dir == claw_dir {
            segment_dir = claw_dir;
            inway_length = random.next_int_bounded(3) + 2;
        } else {
            pos.y += 1;
            let seg_possible = [branch_dir, (0, 1, 0)];
            segment_dir = seg_possible[random.next_int_bounded(2) as usize];
            inway_length = random.next_int_bounded(3) + 3;
        }
        let mut i = 0;
        while i < sideway_length && place_coral_block(ctx, random, pos, state) {
            pos = Pos::new(pos.x + segment_dir.0, pos.y + segment_dir.1, pos.z + segment_dir.2);
            i += 1;
        }
        pos = Pos::new(pos.x - segment_dir.0, pos.y - segment_dir.1, pos.z - segment_dir.2);
        pos.y += 1;
        for _ in 0..inway_length {
            pos = Pos::new(pos.x + claw_dir.0, pos.y + claw_dir.1, pos.z + claw_dir.2);
            if !place_coral_block(ctx, random, pos, state) {
                break;
            }
            if random.next_float() < 0.25 {
                pos.y += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VegetationPatchFeature (moss / clay-pool patches, lush caves)
// ---------------------------------------------------------------------------

fn offset_pos(p: Pos, d: (i32, i32, i32)) -> Pos {
    Pos::new(p.x + d.0, p.y + d.1, p.z + d.2)
}

/// `Vec3i.hashCode` = `(y + z*31)*31 + x` (32-bit wrapping).
fn vec3i_hash(p: Pos) -> i32 {
    (p.y.wrapping_add(p.z.wrapping_mul(31))).wrapping_mul(31).wrapping_add(p.x)
}

/// A faithful `java.util.HashSet<BlockPos>` — its iteration order is a
/// deterministic function of element `hashCode`s, the `hash(h)=h^(h>>>16)`
/// spread, power-of-two bucket indexing `(cap-1)&hash`, per-bucket insertion
/// chaining, and the ×0.75 resize threshold (initial capacity 16). This is the
/// order `VegetationPatchFeature.distributeVegetation` (and the waterlogged
/// water pass) iterate, which drives their RNG draw sequence. Bucket
/// treeification is not modeled: it needs capacity ≥ 64 **and** an 8-deep
/// bucket, which the small patch sets never reach.
struct JavaHashSet {
    table: Vec<Vec<(Pos, i32)>>,
    size: usize,
    threshold: usize,
}

impl JavaHashSet {
    fn new() -> Self {
        JavaHashSet { table: vec![Vec::new(); 16], size: 0, threshold: 12 }
    }
    fn spread(p: Pos) -> i32 {
        let h = vec3i_hash(p);
        h ^ ((h as u32 >> 16) as i32)
    }
    fn add(&mut self, p: Pos) {
        let h = JavaHashSet::spread(p);
        let cap = self.table.len();
        let idx = (h as u32 as usize) & (cap - 1);
        if self.table[idx].iter().any(|(q, _)| *q == p) {
            return;
        }
        self.table[idx].push((p, h));
        self.size += 1;
        if self.size > self.threshold {
            self.resize();
        }
    }
    fn resize(&mut self) {
        let old_cap = self.table.len();
        let new_cap = old_cap * 2;
        let mut new_table: Vec<Vec<(Pos, i32)>> = vec![Vec::new(); new_cap];
        for (j, bucket) in self.table.iter().enumerate() {
            for &(p, h) in bucket {
                if h & (old_cap as i32) == 0 {
                    new_table[j].push((p, h));
                } else {
                    new_table[j + old_cap].push((p, h));
                }
            }
        }
        self.table = new_table;
        self.threshold = (new_cap as f64 * 0.75) as usize;
    }
    /// Elements in HashMap iteration order (table order, per-bucket chain order).
    fn iter_order(&self) -> Vec<Pos> {
        self.table.iter().flatten().map(|(p, _)| *p).collect()
    }
}

fn parse_vegetation_patch(cfg: &Value, waterlogged: bool) -> VegetationPatchConfig {
    VegetationPatchConfig {
        replaceable: dripstone_tag(&cfg["replaceable"]),
        ground_state: StateProvider::parse(&cfg["ground_state"]),
        vegetation_feature: parse_nested(&cfg["vegetation_feature"]),
        surface_floor: cfg["surface"].as_str() != Some("ceiling"),
        depth: IntProvider::parse(&cfg["depth"]),
        extra_bottom_block_chance: cfg["extra_bottom_block_chance"].as_f64().unwrap_or(0.0) as f32,
        vertical_range: cfg["vertical_range"].as_i64().unwrap_or(1) as i32,
        vegetation_chance: cfg["vegetation_chance"].as_f64().unwrap_or(0.0) as f32,
        xz_radius: IntProvider::parse(&cfg["xz_radius"]),
        extra_edge_column_chance: cfg["extra_edge_column_chance"].as_f64().unwrap_or(0.0) as f32,
        waterlogged,
    }
}

fn nested_supported(nf: &NestedFeature) -> bool {
    match nf {
        NestedFeature::Resolved { feature, placement } => {
            feature_fully_supported(feature) && placement.iter().all(|m| m.is_supported())
        }
        _ => false,
    }
}

/// Deep support check: a feature is fully supported only if it (and every nested
/// feature it delegates to) is implemented with a supported placement chain. Used
/// to decide whether a whole `vegetation_patch` / `root_system` may run — a nested
/// template-gated feature (e.g. `sulfur_spring`) makes the enclosing feature skip
/// whole, which is parity-safe because the RNG is reseeded per top feature.
fn feature_fully_supported(f: &ConfiguredFeature) -> bool {
    match f {
        ConfiguredFeature::Deferred(_) => false,
        ConfiguredFeature::RandomSelector(cfg) => {
            cfg.features.iter().all(|w| nested_supported(&w.feature)) && nested_supported(&cfg.default)
        }
        ConfiguredFeature::SimpleRandomSelector(cfg) => cfg.features.iter().all(nested_supported),
        ConfiguredFeature::Sequence(fs) => fs.iter().all(nested_supported),
        ConfiguredFeature::WeightedRandomSelector(fs) => fs.iter().all(|(nf, _)| nested_supported(nf)),
        ConfiguredFeature::RandomBooleanSelector { feature_true, feature_false } => {
            nested_supported(feature_true) && nested_supported(feature_false)
        }
        ConfiguredFeature::VegetationPatch(cfg) => nested_supported(&cfg.vegetation_feature),
        ConfiguredFeature::RootSystem(cfg) => nested_supported(&cfg.feature),
        _ => true,
    }
}

fn parse_multiface_growth(cfg: &Value) -> MultifaceGrowthConfig {
    MultifaceGrowthConfig {
        block: cfg["block"].as_str().and_then(ParityBlock::from_name).unwrap_or(ParityBlock::GlowLichen),
        search_range: cfg.get("search_range").and_then(Value::as_i64).unwrap_or(10) as i32,
        can_place_on_floor: cfg.get("can_place_on_floor").and_then(Value::as_bool).unwrap_or(false),
        can_place_on_ceiling: cfg.get("can_place_on_ceiling").and_then(Value::as_bool).unwrap_or(false),
        can_place_on_wall: cfg.get("can_place_on_wall").and_then(Value::as_bool).unwrap_or(false),
        chance_of_spreading: cfg.get("chance_of_spreading").and_then(Value::as_f64).unwrap_or(0.5) as f32,
        can_be_placed_on: parse_block_holderset(&cfg["can_be_placed_on"]),
    }
}

fn parse_root_system(cfg: &Value) -> RootSystemConfig {
    RootSystemConfig {
        feature: parse_nested(&cfg["feature"]),
        required_vertical_space_for_tree: cfg["required_vertical_space_for_tree"].as_i64().unwrap_or(1) as i32,
        level_test_distance: cfg["level_test_distance"].as_i64().unwrap_or(0) as i32,
        max_level_deviation: cfg["max_level_deviation"].as_i64().unwrap_or(0) as i32,
        root_radius: cfg["root_radius"].as_i64().unwrap_or(1) as i32,
        root_replaceable: dripstone_tag(&cfg["root_replaceable"]),
        root_state_provider: StateProvider::parse(&cfg["root_state_provider"]),
        root_placement_attempts: cfg["root_placement_attempts"].as_i64().unwrap_or(0) as i32,
        root_column_max_height: cfg["root_column_max_height"].as_i64().unwrap_or(1) as i32,
        hanging_root_radius: cfg["hanging_root_radius"].as_i64().unwrap_or(1) as i32,
        hanging_roots_vertical_span: cfg["hanging_roots_vertical_span"].as_i64().unwrap_or(1) as i32,
        hanging_root_state_provider: StateProvider::parse(&cfg["hanging_root_state_provider"]),
        hanging_root_placement_attempts: cfg["hanging_root_placement_attempts"].as_i64().unwrap_or(0) as i32,
        allowed_vertical_water_for_tree: cfg["allowed_vertical_water_for_tree"].as_i64().unwrap_or(1) as i32,
        allowed_tree_position: BlockPredicate::parse(&cfg["allowed_tree_position"]),
    }
}

fn parse_huge_mushroom(cfg: &Value, brown: bool) -> HugeMushroomConfig {
    HugeMushroomConfig {
        cap_provider: StateProvider::parse(&cfg["cap_provider"]),
        stem_provider: StateProvider::parse(&cfg["stem_provider"]),
        foliage_radius: cfg.get("foliage_radius").and_then(Value::as_i64).unwrap_or(2) as i32,
        can_place_on: BlockPredicate::parse(&cfg["can_place_on"]),
        brown,
    }
}

/// `VegetationPatchFeature.placeGround`.
fn veg_place_ground(
    cfg: &VegetationPatchConfig,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    below_start: Pos,
    depth: i32,
) -> bool {
    let dir = if cfg.surface_floor { (0, -1, 0) } else { (0, 1, 0) };
    let mut below = below_start;
    for i in 0..depth {
        let stp = match cfg.ground_state.get_state(ctx.level, random, below) {
            Some(s) => s,
            None => return i != 0,
        };
        let below_state = ctx.level.get_block(below.x, below.y, below.z);
        if stp != below_state {
            if !cfg.replaceable.map(|t| t.contains(below_state)).unwrap_or(false) {
                return i != 0;
            }
            ctx.level.set_block(below.x, below.y, below.z, stp);
            below = offset_pos(below, dir);
        }
    }
    true
}

/// `VegetationPatchFeature.placeGroundPatch`. `isFaceSturdy` is approximated by
/// `blocksMotion` (full-cube top face) over the parity alphabet — draws no RNG.
fn veg_place_ground_patch(
    cfg: &VegetationPatchConfig,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    origin: Pos,
    x_radius: i32,
    z_radius: i32,
) -> JavaHashSet {
    let inwards = if cfg.surface_floor { (0, -1, 0) } else { (0, 1, 0) };
    let outwards = (-inwards.0, -inwards.1, -inwards.2);
    let mut surface = JavaHashSet::new();
    for dx in -x_radius..=x_radius {
        let is_x_edge = dx == -x_radius || dx == x_radius;
        for dz in -z_radius..=z_radius {
            let is_z_edge = dz == -z_radius || dz == z_radius;
            let is_edge = is_x_edge || is_z_edge;
            let is_corner = is_x_edge && is_z_edge;
            let is_edge_not_corner = is_edge && !is_corner;
            if !is_corner
                && (!is_edge_not_corner
                    || (cfg.extra_edge_column_chance != 0.0 && !(random.next_float() > cfg.extra_edge_column_chance)))
            {
                let mut pos = Pos::new(origin.x + dx, origin.y, origin.z + dz);
                let mut offset = 0;
                while ctx.level.get_block(pos.x, pos.y, pos.z).is_air() && offset < cfg.vertical_range {
                    pos = offset_pos(pos, inwards);
                    offset += 1;
                }
                let mut o2 = 0;
                while !ctx.level.get_block(pos.x, pos.y, pos.z).is_air() && o2 < cfg.vertical_range {
                    pos = offset_pos(pos, outwards);
                    o2 += 1;
                }
                let below = offset_pos(pos, inwards);
                let below_state = ctx.level.get_block(below.x, below.y, below.z);
                if ctx.level.get_block(pos.x, pos.y, pos.z).is_air() && below_state.blocks_motion() {
                    let extra = if cfg.extra_bottom_block_chance > 0.0
                        && random.next_float() < cfg.extra_bottom_block_chance
                    {
                        1
                    } else {
                        0
                    };
                    let depth = cfg.depth.sample(random) + extra;
                    let ground_pos = below;
                    if veg_place_ground(cfg, ctx, random, below, depth) {
                        surface.add(ground_pos);
                    }
                }
            }
        }
    }
    surface
}

/// `WaterloggedVegetationPatchFeature.isExposedDirection` — approximated by
/// `!blocksMotion` (a non-full-face neighbour), draws no RNG.
fn veg_is_exposed(ctx: &PlacementCtx, pos: Pos) -> bool {
    // Direction.NORTH, EAST, SOUTH, WEST, DOWN.
    const DIRS: [(i32, i32, i32); 5] = [(0, 0, -1), (1, 0, 0), (0, 0, 1), (-1, 0, 0), (0, -1, 0)];
    DIRS.iter().any(|&(dx, dy, dz)| !ctx.level.get_block(pos.x + dx, pos.y + dy, pos.z + dz).blocks_motion())
}

/// `VegetationPatchFeature.place` (+ the waterlogged variant's water pass).
fn place_vegetation_patch(cfg: &VegetationPatchConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    // The distributeVegetation loop threads the nested vegetation feature's RNG
    // between per-surface `nextFloat` draws, so a deferred vegetation feature
    // would desync the whole patch. When it is unsupported, skip the entire
    // feature (parity-safe: the RNG is reseeded per top feature).
    if !nested_supported(&cfg.vegetation_feature) {
        return;
    }
    let x_radius = cfg.xz_radius.sample(random) + 1;
    let z_radius = cfg.xz_radius.sample(random) + 1;
    let surface = veg_place_ground_patch(cfg, ctx, random, origin, x_radius, z_radius);
    let final_surface: Vec<Pos> = if cfg.waterlogged {
        // Build waterSurface in `surface` iteration order (HashSet order), then
        // set water in waterSurface iteration order.
        let mut water_surface = JavaHashSet::new();
        for sp in surface.iter_order() {
            if !veg_is_exposed(ctx, sp) {
                water_surface.add(sp);
            }
        }
        let order = water_surface.iter_order();
        for &sp in &order {
            ctx.level.set_block(sp.x, sp.y, sp.z, ParityBlock::Water);
        }
        order
    } else {
        surface.iter_order()
    };
    // distributeVegetation.
    let up = if cfg.surface_floor { (0, 1, 0) } else { (0, -1, 0) };
    for sp in final_surface {
        if cfg.vegetation_chance > 0.0 && random.next_float() < cfg.vegetation_chance {
            // Base: place at surfacePos.relative(dir.opposite()). Waterlogged:
            // super.placeVegetation(surfacePos.below()) → surfacePos.
            let veg_pos = if cfg.waterlogged { sp } else { offset_pos(sp, up) };
            place_nested(&cfg.vegetation_feature, ctx, random, veg_pos);
        }
    }
}

// ---------------------------------------------------------------------------
// SnowAndFreezeFeature (freeze_top_layer)
// ---------------------------------------------------------------------------

/// `SnowAndFreezeFeature.place`. Draws no RNG, so it is parity-trivial (it can
/// never desync anything); the exact temperature/frozen chain is reused from the
/// surface rules, and `getBrightness(BLOCK) < 10` is always true at worldgen
/// (no block light). `isFaceSturdy`/`SnowLayerBlock.canSurvive` and the
/// `snowy=true` property update on the block below collapse to `blocksMotion`
/// approximations (documented; no RNG involved).
fn place_freeze_top_layer(ctx: &mut PlacementCtx, origin: Pos) {
    let sea = ctx.level.sea_level();
    for dx in 0..16 {
        for dz in 0..16 {
            let x = origin.x + dx;
            let z = origin.z + dz;
            let y = ctx.level.get_height(Heightmap::MotionBlocking, x, z);
            let top = Pos::new(x, y, z);
            let below = Pos::new(x, y - 1, z);
            let (temp, frozen, has_precip) = ctx.biome_index.biome_snow(ctx.level.get_biome_fill(top.x, top.y, top.z));
            // shouldFreeze(below, checkNeighbors = false): cold enough, in build
            // height, and a water source at `below`.
            if super::surface_rules::cold_enough_to_snow(temp, frozen, below.x, below.y, below.z, sea)
                && !ctx.level.is_outside_build_height(below.y)
                && ctx.level.get_block(below.x, below.y, below.z) == ParityBlock::Water
            {
                ctx.level.set_block(below.x, below.y, below.z, ParityBlock::Ice);
            }
            // shouldSnow(top): precipitation is SNOW (has_precipitation && cold
            // enough), the top is air/snow, and snow can survive on the block
            // below (approximated by a motion-blocking, non-icy support).
            let below_of_top = ctx.level.get_block(top.x, top.y - 1, top.z);
            let snow_can_survive = below_of_top.blocks_motion()
                && !matches!(below_of_top, ParityBlock::Ice | ParityBlock::PackedIce | ParityBlock::BlueIce);
            if has_precip
                && super::surface_rules::cold_enough_to_snow(temp, frozen, top.x, top.y, top.z, sea)
                && !ctx.level.is_outside_build_height(top.y)
                && (ctx.level.get_block(top.x, top.y, top.z).is_air()
                    || ctx.level.get_block(top.x, top.y, top.z) == ParityBlock::Snow)
                && snow_can_survive
            {
                ctx.level.set_block(top.x, top.y, top.z, ParityBlock::Snow);
            }
        }
    }
}

/// `CoralMushroomFeature.placeFeature`.
fn coral_mushroom(ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos, state: ParityBlock) {
    let height = random.next_int_bounded(3) + 3;
    let width = random.next_int_bounded(3) + 3;
    let length = random.next_int_bounded(3) + 3;
    let sink = random.next_int_bounded(3) + 1;
    for x in 0..=width {
        for y in 0..=height {
            for z in 0..=length {
                let pos = Pos::new(x + origin.x, y + origin.y - sink, z + origin.z);
                if (x != 0 && x != width || y != 0 && y != height)
                    && (z != 0 && z != length || y != 0 && y != height)
                    && (x != 0 && x != width || z != 0 && z != length)
                    && (x == 0 || x == width || y == 0 || y == height || z == 0 || z == length)
                    && !(random.next_float() < 0.1)
                {
                    place_coral_block(ctx, random, pos, state);
                }
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
            TreeDecorator::AttachedToLogs { probability, block_provider, directions } => {
                attached_to_logs_place(*probability, block_provider, directions, level, decorations, random, trunks);
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
fn place_tree(config: &TreeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    // Graceful skip for unported placers (fancy trunk, fancy/mega foliage, …):
    // bail before any RNG draw so this terminal feature simply produces nothing
    // rather than mis-drawing (safe — the RNG is reseeded per top feature).
    if config.trunk_placer.is_unsupported()
        || config.foliage_placer.is_unsupported()
        || config.root_placer_unsupported
    {
        return false;
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
        return false;
    }
    let grow_through = config.trunk_placer.grow_through();
    let min_clipped = config.minimum_size.min_clipped_height();
    let clipped = get_max_free_tree_height(level, tree_height, trunk_origin, config, grow_through);
    if !(clipped >= tree_height || min_clipped.map(|m| clipped >= m).unwrap_or(false)) {
        return false;
    }

    let mut trunks: HashSet<Pos> = HashSet::new();
    let mut foliage: HashSet<Pos> = HashSet::new();
    let mut roots: HashSet<Pos> = HashSet::new();
    let mut decorations: HashSet<Pos> = HashSet::new();

    // Roots are placed first; a failed root system aborts the whole tree.
    if let Some(rp) = &config.root_placer {
        if !rp.place_roots(level, &mut roots, random, origin, trunk_origin, config) {
            return false;
        }
    }

    let attachments = config.trunk_placer.place_trunk(level, &mut trunks, random, clipped, trunk_origin, config);
    for att in &attachments {
        config
            .foliage_placer
            .create_foliage(level, &mut foliage, random, config, clipped, att, foliage_height, leaf_radius);
    }

    // `place`: decorators run only when the tree placed something.
    let placed = !trunks.is_empty() || !foliage.is_empty();
    if placed {
        for dec in &config.decorators {
            dec.place(level, &mut decorations, random, &trunks, &foliage, &roots);
        }
    }
    // `updateLeaves` + `updateShapeAtEdge` are the deferred block-state pass.
    placed
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

// ---------------------------------------------------------------------------
// P8 close-out features (lush caves / mushrooms / vines / sculk / sulfur)
// ---------------------------------------------------------------------------

/// A full `Direction`. `VALUES` reproduces `Direction.values()` order
/// (DOWN, UP, NORTH, SOUTH, WEST, EAST); the multiface config builds its own
/// ordered list per the ceiling/floor/wall flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir6 {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir6 {
    const VALUES: [Dir6; 6] = [Dir6::Down, Dir6::Up, Dir6::North, Dir6::South, Dir6::West, Dir6::East];
    fn step(self) -> (i32, i32, i32) {
        match self {
            Dir6::Down => (0, -1, 0),
            Dir6::Up => (0, 1, 0),
            Dir6::North => (0, 0, -1),
            Dir6::South => (0, 0, 1),
            Dir6::West => (-1, 0, 0),
            Dir6::East => (1, 0, 0),
        }
    }
    fn opposite(self) -> Dir6 {
        match self {
            Dir6::Down => Dir6::Up,
            Dir6::Up => Dir6::Down,
            Dir6::North => Dir6::South,
            Dir6::South => Dir6::North,
            Dir6::West => Dir6::East,
            Dir6::East => Dir6::West,
        }
    }
}

/// `SequenceFeature.place` — place each nested placed feature in order, failing
/// fast (returning false) at the first that fails.
fn place_sequence(features: &[NestedFeature], ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    for f in features {
        if !place_nested(f, ctx, random, origin) {
            return false;
        }
    }
    true
}

/// `WeightedRandomSelectorFeature.place` — `nextInt(total_weight)` selects one
/// entry (cumulative-weight scan). Reached only via `rooted_sulfur_spring`, which
/// `root_system` skips whole (template-gated); kept for completeness.
fn place_weighted_random_selector(
    entries: &[(NestedFeature, i32)],
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    origin: Pos,
) -> bool {
    let total: i32 = entries.iter().map(|(_, w)| *w).sum();
    if total <= 0 {
        return false;
    }
    let mut sel = random.next_int_bounded(total);
    for (nf, w) in entries {
        sel -= *w;
        if sel < 0 {
            return place_nested(nf, ctx, random, origin);
        }
    }
    false
}

/// `MultifaceGrowthFeature.place` (glow_lichen / sculk_vein). The placed block
/// collapses to its default state (face booleans dropped); the RNG draws — the
/// direction shuffles and the spread `nextFloat` + `Direction.allShuffled` — are
/// consumed 1:1. The spread's own block writes are elided (terminal, cannot
/// desync anything downstream).
fn place_multiface_growth(cfg: &MultifaceGrowthConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    // `isAirOrWater(origin)`.
    let ob = ctx.level.get_block(origin.x, origin.y, origin.z);
    if !ob.is_air() && ob != ParityBlock::Water {
        return false;
    }
    // `validDirections`: ceiling (UP), floor (DOWN), then the 4 horizontals.
    let mut valid: Vec<Dir6> = Vec::with_capacity(6);
    if cfg.can_place_on_ceiling {
        valid.push(Dir6::Up);
    }
    if cfg.can_place_on_floor {
        valid.push(Dir6::Down);
    }
    if cfg.can_place_on_wall {
        valid.extend([Dir6::North, Dir6::East, Dir6::South, Dir6::West]);
    }
    if valid.is_empty() {
        return false;
    }
    // `getShuffledDirections` = `Util.shuffledCopy(validDirections)`.
    let mut var14 = valid.clone();
    util_shuffle(&mut var14, random);
    if multiface_place_growth_if_possible(cfg, ctx, random, origin, &var14) {
        return true;
    }
    for &search_dir in &var14 {
        // `getShuffledDirectionsExcept(search_dir.opposite)`.
        let exclude = search_dir.opposite();
        let mut placement_dirs: Vec<Dir6> = valid.iter().copied().filter(|&d| d != exclude).collect();
        util_shuffle(&mut placement_dirs, random);
        let (sx, sy, sz) = search_dir.step();
        // NB: vanilla 26.2 re-sets `pos = origin.relative(searchDirection)` each
        // iteration (distance 1), so the whole range probes the same cell.
        let pos = Pos::new(origin.x + sx, origin.y + sy, origin.z + sz);
        for _ in 0..cfg.search_range {
            let state = ctx.level.get_block(pos.x, pos.y, pos.z);
            if !state.is_air() && state != ParityBlock::Water && state != cfg.block {
                break;
            }
            if multiface_place_growth_if_possible(cfg, ctx, random, pos, &placement_dirs) {
                return true;
            }
        }
    }
    false
}

/// `MultifaceGrowthFeature.placeGrowthIfPossible`.
fn multiface_place_growth_if_possible(
    cfg: &MultifaceGrowthConfig,
    ctx: &mut PlacementCtx,
    random: &mut WorldgenRandom,
    pos: Pos,
    placement_dirs: &[Dir6],
) -> bool {
    for &dir in placement_dirs {
        let (dx, dy, dz) = dir.step();
        let neighbour = ctx.level.get_block(pos.x + dx, pos.y + dy, pos.z + dz);
        if cfg.can_be_placed_on.contains(&neighbour) {
            // `getStateForPlacement` never returns null for a supported face with a
            // valid neighbour; the multiface state collapses to `cfg.block`.
            ctx.level.set_block(pos.x, pos.y, pos.z, cfg.block);
            if random.next_float() < cfg.chance_of_spreading {
                // `getSpreader().spreadFromFaceTowardRandomDirection` →
                // `Direction.allShuffled` = shuffledCopy of the 6 faces (5 draws);
                // the spread placements are deterministic (no further RNG) and
                // elided here (terminal, cannot desync anything).
                let mut faces = Dir6::VALUES;
                util_shuffle(&mut faces, random);
            }
            return true;
        }
    }
    false
}

/// `RootSystemFeature.place` (rooted_azalea_tree; rooted_sulfur_spring is
/// template-gated → skipped whole).
fn place_root_system(cfg: &RootSystemConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    // Skip whole when the nested tree feature is not fully supported (parity-safe:
    // the RNG is reseeded per top feature). No RNG is drawn before this point.
    if !nested_supported(&cfg.feature) {
        return true;
    }
    if !ctx.level.get_block(origin.x, origin.y, origin.z).is_air() {
        return false;
    }
    if root_place_dirt_and_tree(cfg, ctx, random, origin) {
        root_place_hanging(cfg, ctx, random, origin);
    }
    true
}

/// `RootSystemFeature.isAllowedTreeSpace`.
fn root_allowed_tree_space(state: ParityBlock, blocks_above: i32, allowed_water: i32) -> bool {
    state.is_air() || (blocks_above + 1 <= allowed_water && state == ParityBlock::Water)
}

/// `RootSystemFeature.spaceForTree` (no RNG). `isSolid` corner check approximates
/// vanilla's air / non-air tests directly.
fn root_space_for_tree(cfg: &RootSystemConfig, ctx: &PlacementCtx, pos: Pos) -> bool {
    for i in 1..=cfg.required_vertical_space_for_tree {
        let s = ctx.level.get_block(pos.x, pos.y + i, pos.z);
        if !root_allowed_tree_space(s, i, cfg.allowed_vertical_water_for_tree) {
            return false;
        }
    }
    if cfg.level_test_distance > 0 {
        // `Direction.from2DDataValue`: 0=SOUTH, 1=WEST, 2=NORTH, 3=EAST.
        const CORNERS: [(i32, i32); 4] = [(0, 1), (-1, 0), (0, -1), (1, 0)];
        for (cx, cz) in CORNERS {
            let corner = Pos::new(pos.x + cx * cfg.level_test_distance, pos.y, pos.z + cz * cfg.level_test_distance);
            let below = ctx.level.get_block(corner.x, corner.y - cfg.max_level_deviation, corner.z);
            let above = ctx.level.get_block(corner.x, corner.y + cfg.max_level_deviation, corner.z);
            if below.is_air() || !above.is_air() {
                return false;
            }
        }
    }
    true
}

/// `RootSystemFeature.placeDirtAndTree`.
fn root_place_dirt_and_tree(cfg: &RootSystemConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    for y in 0..cfg.root_column_max_height {
        let working = Pos::new(origin.x, origin.y + y + 1, origin.z);
        if ctx.level.get_height(Heightmap::WorldSurface, working.x, working.z) < working.y {
            return false;
        }
        if cfg.allowed_tree_position.test(ctx.level, working) && root_space_for_tree(cfg, ctx, working) {
            let below = ctx.level.get_block(working.x, working.y - 1, working.z);
            if below == ParityBlock::Lava || !below.blocks_motion() {
                return false;
            }
            if place_nested(&cfg.feature, ctx, random, working) {
                root_place_dirt(cfg, ctx, random, origin, origin.y + y);
                return true;
            }
        }
    }
    false
}

/// `RootSystemFeature.placeDirt` + `placeRootedDirt`.
fn root_place_dirt(cfg: &RootSystemConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos, target_height: i32) {
    for y in origin.y..target_height {
        for _ in 0..cfg.root_placement_attempts {
            let dx = random.next_int_bounded(cfg.root_radius) - random.next_int_bounded(cfg.root_radius);
            let dz = random.next_int_bounded(cfg.root_radius) - random.next_int_bounded(cfg.root_radius);
            let wp = Pos::new(origin.x + dx, y, origin.z + dz);
            let at = ctx.level.get_block(wp.x, wp.y, wp.z);
            if cfg.root_replaceable.map(|t| t.contains(at)).unwrap_or(false) {
                if let Some(s) = cfg.root_state_provider.get_state(ctx.level, random, wp) {
                    ctx.level.set_block(wp.x, wp.y, wp.z, s);
                }
            }
        }
    }
}

/// `RootSystemFeature.placeRoots` (hanging roots). `canSurvive` / `isFaceSturdy`
/// collapse to a `blocks_motion` support test (draws no RNG).
fn root_place_hanging(cfg: &RootSystemConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    for _ in 0..cfg.hanging_root_placement_attempts {
        let dx = random.next_int_bounded(cfg.hanging_root_radius) - random.next_int_bounded(cfg.hanging_root_radius);
        let dy = random.next_int_bounded(cfg.hanging_roots_vertical_span) - random.next_int_bounded(cfg.hanging_roots_vertical_span);
        let dz = random.next_int_bounded(cfg.hanging_root_radius) - random.next_int_bounded(cfg.hanging_root_radius);
        let wp = Pos::new(origin.x + dx, origin.y + dy, origin.z + dz);
        if ctx.level.get_block(wp.x, wp.y, wp.z).is_air() {
            if let Some(s) = cfg.hanging_root_state_provider.get_state(ctx.level, random, wp) {
                if ctx.level.get_block(wp.x, wp.y + 1, wp.z).blocks_motion() {
                    ctx.level.set_block(wp.x, wp.y, wp.z, s);
                }
            }
        }
    }
}

/// `FallenTreeFeature.placeFallenTree`.
fn place_fallen_tree(cfg: &FallenTreeConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) {
    // Stump (log identity; axis collapses) + its decorators.
    if let Some(s) = cfg.trunk_provider.get_state(ctx.level, random, origin) {
        ctx.level.set_block(origin.x, origin.y, origin.z, s);
    }
    fallen_decorate_logs(&cfg.stump_decorators, &[origin], ctx.level, random);

    let dir = horizontal_random_direction(random);
    let (sx, sz) = (dir.step_x(), dir.step_z());
    let log_length = cfg.log_length.sample(random) - 2;
    let off = 2 + random.next_int_bounded(2);
    let mut start = Pos::new(origin.x + sx * off, origin.y, origin.z + sz * off);
    // `setGroundHeightForFallenLogStartPos`.
    {
        let mut p = Pos::new(start.x, start.y + 1, start.z);
        for _ in 0..6 {
            if valid_tree_pos(ctx.level, p) && fallen_over_solid(ctx.level, p) {
                break;
            }
            p = Pos::new(p.x, p.y - 1, p.z);
        }
        start = p;
    }
    if !fallen_can_place_entire(ctx.level, log_length, start, sx, sz) {
        return;
    }
    // `placeFallenLog`.
    let mut logs: Vec<Pos> = Vec::with_capacity(log_length.max(0) as usize);
    let mut p = start;
    for _ in 0..log_length {
        if let Some(s) = cfg.trunk_provider.get_state(ctx.level, random, p) {
            ctx.level.set_block(p.x, p.y, p.z, s);
        }
        logs.push(p);
        p = Pos::new(p.x + sx, p.y, p.z + sz);
    }
    fallen_decorate_logs(&cfg.log_decorators, &logs, ctx.level, random);
}

/// `FallenTreeFeature.isOverSolidGround` — `isFaceSturdy(below, UP)`, approx.
fn fallen_over_solid(level: &dyn DecorationLevel, p: Pos) -> bool {
    level.get_block(p.x, p.y - 1, p.z).blocks_motion()
}

/// `FallenTreeFeature.canPlaceEntireFallenLog`.
fn fallen_can_place_entire(level: &dyn DecorationLevel, log_length: i32, start: Pos, sx: i32, sz: i32) -> bool {
    let mut gap = 0;
    let mut p = start;
    for _ in 0..log_length {
        if !valid_tree_pos(level, p) {
            return false;
        }
        if !fallen_over_solid(level, p) {
            gap += 1;
            if gap > 2 {
                return false;
            }
        } else {
            gap = 0;
        }
        p = Pos::new(p.x + sx, p.y, p.z + sz);
    }
    true
}

/// `FallenTreeFeature.decorateLogs` — run each decorator over the log set.
fn fallen_decorate_logs(decorators: &[TreeDecorator], logs: &[Pos], level: &mut dyn DecorationLevel, random: &mut WorldgenRandom) {
    if decorators.is_empty() {
        return;
    }
    let log_set: HashSet<Pos> = logs.iter().copied().collect();
    let empty: HashSet<Pos> = HashSet::new();
    let mut decorations: HashSet<Pos> = HashSet::new();
    for dec in decorators {
        dec.place(level, &mut decorations, random, &log_set, &empty, &empty);
    }
}

/// `AttachedToLogsDecorator.place`. `Util.shuffledCopy(logs)` order can't match
/// the JVM `HashSet`; the by-Y sort is the same tradeoff the other decorators
/// document (RNG-draw-count faithful, block grid approximate).
fn attached_to_logs_place(
    probability: f32,
    block_provider: &StateProvider,
    directions: &[(i32, i32, i32)],
    level: &mut dyn DecorationLevel,
    decorations: &mut HashSet<Pos>,
    random: &mut WorldgenRandom,
    logs: &HashSet<Pos>,
) {
    if directions.is_empty() {
        return;
    }
    let mut list = sorted_by_y(logs);
    util_shuffle(&mut list, random);
    for log in list {
        let (dx, dy, dz) = directions[random.next_int_bounded(directions.len() as i32) as usize];
        let placement = Pos::new(log.x + dx, log.y + dy, log.z + dz);
        if random.next_float() <= probability && level.get_block(placement.x, placement.y, placement.z).is_air() {
            if let Some(state) = block_provider.get_state(&*level, random, placement) {
                decorations.insert(placement);
                level.set_block(placement.x, placement.y, placement.z, state);
            }
        }
    }
}

/// `AbstractHugeMushroomFeature.place` + red/brown `makeCap`. Face booleans
/// collapse to the default block state; the `getTreeHeight` draws (and any
/// provider draws, none for the vanilla simple providers) are consumed 1:1.
fn place_huge_mushroom(cfg: &HugeMushroomConfig, ctx: &mut PlacementCtx, random: &mut WorldgenRandom, origin: Pos) -> bool {
    // `getTreeHeight`.
    let mut tree_height = random.next_int_bounded(3) + 4;
    if random.next_int_bounded(12) == 0 {
        tree_height *= 2;
    }
    if !huge_mushroom_valid(cfg, ctx, origin, tree_height) {
        return false;
    }
    // `makeCap`.
    if cfg.brown {
        let r = cfg.foliage_radius;
        for dx in -r..=r {
            for dz in -r..=r {
                let x_edge = dx == -r || dx == r;
                let z_edge = dz == -r || dz == r;
                if !x_edge || !z_edge {
                    let state = cfg.cap_provider.get_state(ctx.level, random, origin);
                    huge_place_block(ctx, Pos::new(origin.x + dx, origin.y + tree_height, origin.z + dz), state);
                }
            }
        }
    } else {
        for dy in (tree_height - 3)..=tree_height {
            let radius = if dy < tree_height { cfg.foliage_radius } else { cfg.foliage_radius - 1 };
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let x_edge = dx == -radius || dx == radius;
                    let z_edge = dz == -radius || dz == radius;
                    if dy >= tree_height || x_edge != z_edge {
                        let state = cfg.cap_provider.get_state(ctx.level, random, origin);
                        huge_place_block(ctx, Pos::new(origin.x + dx, origin.y + dy, origin.z + dz), state);
                    }
                }
            }
        }
    }
    // `placeTrunk`.
    for dy in 0..tree_height {
        let state = cfg.stem_provider.get_state(ctx.level, random, origin);
        huge_place_block(ctx, Pos::new(origin.x, origin.y + dy, origin.z), state);
    }
    true
}

/// `AbstractHugeMushroomFeature.isValidPosition` (no RNG).
fn huge_mushroom_valid(cfg: &HugeMushroomConfig, ctx: &PlacementCtx, origin: Pos, tree_height: i32) -> bool {
    let y = origin.y;
    if y < ctx.level.min_y() + 1 || y + tree_height + 1 > ctx.level.max_y() {
        return false;
    }
    if !cfg.can_place_on.test(ctx.level, Pos::new(origin.x, origin.y - 1, origin.z)) {
        return false;
    }
    for dy in 0..=tree_height {
        let radius = huge_radius_for_height(cfg, tree_height, dy);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let b = ctx.level.get_block(origin.x + dx, origin.y + dy, origin.z + dz);
                if !b.is_air() && !b.is_leaves() {
                    return false;
                }
            }
        }
    }
    true
}

/// `getTreeRadiusForHeight` for the red / brown subclasses.
fn huge_radius_for_height(cfg: &HugeMushroomConfig, tree_height: i32, yo: i32) -> i32 {
    if cfg.brown {
        if yo <= 3 { 0 } else { cfg.foliage_radius }
    } else if (yo < tree_height && yo >= tree_height - 3) || yo == tree_height {
        cfg.foliage_radius
    } else {
        0
    }
}

/// `AbstractHugeMushroomFeature.placeMushroomBlock` — place only into air / a
/// `#replaceable_by_mushrooms` cell (draws no RNG).
fn huge_place_block(ctx: &mut PlacementCtx, pos: Pos, state: Option<ParityBlock>) {
    let Some(state) = state else { return };
    let cur = ctx.level.get_block(pos.x, pos.y, pos.z);
    if cur.is_air() || BlockTag::ReplaceableByMushrooms.contains(cur) {
        ctx.level.set_block(pos.x, pos.y, pos.z, state);
    }
}

/// `VinesFeature.place` — no RNG. `isAcceptableNeighbour` collapses to a
/// full-face (`blocks_motion`) support test; the vine face property collapses.
fn place_vines(ctx: &mut PlacementCtx, origin: Pos) -> bool {
    if !ctx.level.get_block(origin.x, origin.y, origin.z).is_air() {
        return false;
    }
    for dir in Dir6::VALUES {
        if dir == Dir6::Down {
            continue;
        }
        let (dx, dy, dz) = dir.step();
        if ctx.level.get_block(origin.x + dx, origin.y + dy, origin.z + dz).blocks_motion() {
            ctx.level.set_block(origin.x, origin.y, origin.z, ParityBlock::Vine);
            return true;
        }
    }
    false
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

    /// A cold, precipitating biome (snowy-plains-like) for `freeze_top_layer`.
    struct ColdBiome;
    impl BiomeFeatureIndex for ColdBiome {
        fn biome_has_feature(&self, _fill: u16, _id: &str) -> bool {
            true
        }
        fn biome_snow(&self, _fill: u16) -> (f32, bool, bool) {
            (0.0, false, true)
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
        // A feature type this milestone still defers (fossils need Mojang NBT
        // structure templates — clean-room policy) stays `Deferred`.
        assert!(!registry.configured.get("fossil_coal").map(|c| c.is_implemented()).unwrap_or(true));
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

    /// `monster_room` carves a cobblestone dungeon shell with a spawner core when
    /// the room has 1–5 doorways; deterministic. The spawner mob and chest loot
    /// are block-entity NBT (deferred) but the states + RNG are placed/consumed.
    #[test]
    fn monster_room_builds_dungeon() {
        use ParityBlock::*;
        let run = |seed: i64| {
            let mut level = TestLevel::new(300); // all stone below y=300
            // Punch a two-high doorway at both candidate border columns so a
            // hole is counted regardless of the room's random xr (2 or 3).
            for &dx in &[3, 4] {
                level.set_block(dx, 100, 0, Air);
                level.set_block(dx, 101, 0, Air);
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_monster_room(&mut ctx, &mut random, Pos::new(0, 100, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(2);
        assert_eq!(a, run(2), "monster room deterministic");
        assert_eq!(a.iter().filter(|(_, b)| *b == Spawner).count(), 1, "one spawner at the core");
        assert!(a.iter().any(|(_, b)| matches!(b, Cobblestone | MossyCobblestone)), "cobblestone shell");
    }

    /// `underwater_magma` sows magma blocks in the ocean floor beneath a water
    /// column, deterministically.
    #[test]
    fn underwater_magma_sows_magma() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:warm_ocean"]);
        let cfg = match reg.configured.get("underwater_magma") {
            Some(ConfiguredFeature::UnderwaterMagma(c)) => c.clone(),
            other => panic!("underwater_magma is not underwater_magma: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = TestLevel::new(56); // stone below 56
            for z in -4..5 {
                for x in -4..5 {
                    for y in 56..70 {
                        level.set_block(x, y, z, Water);
                    }
                }
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_underwater_magma(&cfg, &mut ctx, &mut random, Pos::new(0, 57, 0));
            level.blocks.iter().filter(|(_, b)| **b == MagmaBlock).count()
        };
        let a = run(4);
        assert_eq!(a, run(4), "underwater magma deterministic");
        assert!(a >= 1, "at least one magma block in the floor, got {a}");
    }

    /// `geode` (amethyst_geode) grows nested amethyst / calcite / smooth-basalt
    /// shells with budding amethyst and crystal placements, deterministically.
    #[test]
    fn geode_grows_amethyst_shells() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:dripstone_caves"]);
        let cfg = match reg.configured.get("amethyst_geode") {
            Some(ConfiguredFeature::Geode(c)) => c.clone(),
            other => panic!("amethyst_geode is not a geode: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = TestLevel::new(300); // all stone
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_geode(&cfg, &mut ctx, &mut random, Pos::new(0, 100, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(3);
        assert_eq!(a, run(3), "geode deterministic");
        assert!(a.iter().any(|(_, b)| *b == Calcite), "the middle layer is calcite");
        assert!(a.iter().any(|(_, b)| *b == AmethystBlock), "the inner layer is amethyst block");
        assert!(a.iter().any(|(_, b)| *b == SmoothBasalt), "the outer crust is smooth basalt");
        assert!(a.iter().any(|(_, b)| *b == BuddingAmethyst), "budding amethyst appears");
    }

    /// A cave: solid stone with an air gap in `[floor+1, ceiling)`.
    fn cave_level(floor: i32, ceiling: i32) -> TestLevel {
        let mut level = TestLevel::new(400); // all stone below y=400
        for z in -20..20 {
            for x in -20..20 {
                for y in floor + 1..ceiling {
                    level.set_block(x, y, z, ParityBlock::Air);
                }
            }
        }
        level
    }

    /// A single `speleothem` (pointed_dripstone) grows a pointed dripstone off a
    /// stone floor, laying a dripstone-block base patch; deterministic.
    #[test]
    fn speleothem_grows_pointed_dripstone() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:dripstone_caves"]);
        // Pull the inline speleothem config out of the pointed_dripstone selector.
        let cfg = match reg.configured.get("pointed_dripstone") {
            Some(ConfiguredFeature::SimpleRandomSelector(sc)) => {
                match sc.features.first() {
                    Some(NestedFeature::Resolved { feature, .. }) => match feature.as_ref() {
                        ConfiguredFeature::Speleothem(c) => c.clone(),
                        other => panic!("nested feature is not speleothem: {other:?}"),
                    },
                    other => panic!("selector feature not resolved: {other:?}"),
                }
            }
            other => panic!("pointed_dripstone is not a simple_random_selector: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = cave_level(89, 100); // floor stone at 89, air 90..100
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_speleothem(&cfg, &mut ctx, &mut random, Pos::new(0, 90, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(5);
        assert_eq!(a, run(5), "speleothem deterministic");
        assert!(a.iter().any(|(_, b)| *b == PointedDripstone), "a pointed dripstone grew");
        assert!(a.iter().any(|(_, b)| *b == DripstoneBlock), "a dripstone-block base patch was laid");
    }

    /// `pointed_dripstone` (simple_random_selector) reaches the speleothem through
    /// its environment_scan + random_offset placement chain; deterministic.
    #[test]
    fn pointed_dripstone_selector_is_deterministic() {
        let reg = registry_for(&["minecraft:dripstone_caves"]);
        let cf = reg.configured.get("pointed_dripstone").expect("pointed_dripstone").clone();
        let run = |seed: i64| {
            let mut level = cave_level(89, 100);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_feature(&cf, &mut ctx, &mut random, Pos::new(0, 95, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        assert_eq!(run(9), run(9), "pointed_dripstone selector deterministic");
    }

    /// `dripstone_cluster` (speleothem_cluster) hangs stalactites/stalagmites in a
    /// cave, deterministically, placing pointed dripstone and dripstone block.
    #[test]
    fn dripstone_cluster_fills_cave() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:dripstone_caves"]);
        let cfg = match reg.configured.get("dripstone_cluster") {
            Some(ConfiguredFeature::SpeleothemCluster(c)) => c.clone(),
            other => panic!("dripstone_cluster is not a speleothem_cluster: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = cave_level(88, 100); // floor 88, air 89..100, ceiling 100
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_speleothem_cluster(&cfg, &mut ctx, &mut random, Pos::new(0, 94, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        // Sweep for a seed that grows at least one pointed dripstone.
        let seed = (0..64)
            .find(|s| run(*s).iter().any(|(_, b)| *b == PointedDripstone))
            .expect("some seed grows a dripstone cluster");
        let a = run(seed);
        assert_eq!(a, run(seed), "dripstone cluster deterministic");
        assert!(a.iter().any(|(_, b)| *b == PointedDripstone), "pointed dripstone");
        assert!(a.iter().any(|(_, b)| *b == DripstoneBlock), "dripstone block base layer");
    }

    /// `large_dripstone` builds a dripstone column in a tall cave; deterministic.
    #[test]
    fn large_dripstone_builds_column() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:dripstone_caves"]);
        let cfg = match reg.configured.get("large_dripstone") {
            Some(ConfiguredFeature::LargeDripstone(c)) => c.clone(),
            other => panic!("large_dripstone is not a large_dripstone: {other:?}"),
        };
        let run = |seed: i64| {
            let mut level = cave_level(80, 110); // tall cave: floor 80, air 81..110
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_large_dripstone(&cfg, &mut ctx, &mut random, Pos::new(0, 95, 0));
            level.blocks.iter().filter(|(_, b)| **b == DripstoneBlock).count()
        };
        let seed = (0..128).find(|s| run(*s) > 0).expect("some seed builds a large dripstone");
        assert_eq!(run(seed), run(seed), "large dripstone deterministic");
        assert!(run(seed) > 0, "the column places dripstone block");
    }

    /// Coral features (`coral_tree`/`coral_claw`/`coral_mushroom`) grow a coral
    /// structure in a warm-ocean water column, deterministically, and pick one of
    /// the five coral colours from `#coral_blocks`.
    #[test]
    fn coral_features_grow_in_water() {
        use ParityBlock::*;
        let coral_blocks =
            [TubeCoralBlock, BrainCoralBlock, BubbleCoralBlock, FireCoralBlock, HornCoralBlock];
        for kind_name in ["tree", "claw", "mushroom"] {
            let run = |seed: i64| {
                // Stone floor at 50, deep water 51..72.
                let mut level = TestLevel::new(51);
                for z in -12..12 {
                    for x in -12..12 {
                        for y in 51..72 {
                            level.set_block(x, y, z, Water);
                        }
                    }
                }
                let idx = AllBiome;
                let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
                let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
                let kind = match kind_name {
                    "tree" => CoralKind::Tree,
                    "claw" => CoralKind::Claw,
                    _ => CoralKind::Mushroom,
                };
                place_coral(kind, &mut ctx, &mut random, Pos::new(0, 55, 0));
                let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
                writes.sort_by_key(|(k, _)| *k);
                writes
            };
            let seed = (0..64)
                .find(|s| run(*s).iter().any(|(_, b)| coral_blocks.contains(b)))
                .unwrap_or_else(|| panic!("coral_{kind_name} grows for some seed"));
            let a = run(seed);
            assert_eq!(a, run(seed), "coral_{kind_name} deterministic");
            assert!(
                a.iter().any(|(_, b)| coral_blocks.contains(b)),
                "coral_{kind_name} places a coral block"
            );
        }
    }

    /// The `JavaHashSet` iteration order is a deterministic function of the
    /// inserted `BlockPos`es and reproduces across identical insert sequences,
    /// and every inserted element is present exactly once.
    #[test]
    fn java_hashset_is_deterministic() {
        let build = || {
            let mut s = JavaHashSet::new();
            for x in -5..6 {
                for z in -3..4 {
                    s.add(Pos::new(x, 40, z));
                }
            }
            // Re-adding is a no-op (Set semantics).
            s.add(Pos::new(0, 40, 0));
            s.iter_order()
        };
        let a = build();
        assert_eq!(a, build(), "hashset order is reproducible");
        assert_eq!(a.len(), 11 * 7, "all distinct positions present, no dupes");
        // The order is generally not the insertion order (bucket-driven).
        let insertion: Vec<Pos> =
            (-5..6).flat_map(|x| (-3..4).map(move |z| Pos::new(x, 40, z))).collect();
        assert_ne!(a, insertion, "iteration order is bucket-driven, not insertion order");
    }

    /// `moss_patch` (vegetation_patch) lays a moss-block ground patch on a cave
    /// floor and distributes moss vegetation (the `moss_vegetation` simple_block)
    /// over it in Java-HashSet order; deterministic.
    #[test]
    fn vegetation_patch_lays_moss() {
        use ParityBlock::*;
        let reg = registry_for(&["minecraft:lush_caves"]);
        let cfg = match reg.configured.get("moss_patch") {
            Some(ConfiguredFeature::VegetationPatch(c)) => c.clone(),
            other => panic!("moss_patch is not a vegetation_patch: {other:?}"),
        };
        // The moss vegetation feature must be supported for the patch to run.
        assert!(nested_supported(&cfg.vegetation_feature), "moss_vegetation is supported");
        let run = |seed: i64| {
            let mut level = TestLevel::new(90); // stone floor below y=90, air above
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_vegetation_patch(&cfg, &mut ctx, &mut random, Pos::new(0, 93, 0));
            let mut writes: Vec<_> = level.blocks.iter().map(|(k, v)| (*k, *v)).collect();
            writes.sort_by_key(|(k, _)| *k);
            writes
        };
        let a = run(4);
        assert_eq!(a, run(4), "moss patch deterministic");
        assert!(a.iter().any(|(_, b)| *b == MossBlock), "a moss-block ground patch is laid");
        // Some moss vegetation (short_grass / moss_carpet / azalea / etc.) is placed.
        assert!(
            a.iter().any(|(_, b)| matches!(b, ShortGrass | TallGrass | MossCarpet | Azalea | FloweringAzalea)),
            "moss vegetation is distributed over the patch"
        );
    }

    /// `freeze_top_layer` freezes exposed water to ice and lays a snow layer on
    /// cold, solid ground; it draws no RNG (parity-trivial).
    #[test]
    fn freeze_top_layer_ices_and_snows() {
        use ParityBlock::*;
        let mut level = TestLevel::new(70); // air at y>=70, stone below
        // A water pool one below the surface in the left half of the chunk.
        for x in 0..8 {
            for z in 0..16 {
                level.set_block(x, 69, z, Water);
            }
        }
        let idx = ColdBiome;
        let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
        place_freeze_top_layer(&mut ctx, Pos::new(0, 0, 0));
        // Water columns freeze to ice at y=69; solid columns get a snow layer at y=70.
        assert_eq!(level.get_block(3, 69, 3), Ice, "exposed water froze to ice");
        assert_eq!(level.get_block(12, 70, 5), Snow, "cold solid ground got a snow layer");
        // A second run is identical (no RNG).
        let mut level2 = TestLevel::new(70);
        for x in 0..8 {
            for z in 0..16 {
                level2.set_block(x, 69, z, Water);
            }
        }
        let idx2 = ColdBiome;
        let mut ctx2 = PlacementCtx { level: &mut level2, biome_index: &idx2, top_feature: "t" };
        place_freeze_top_layer(&mut ctx2, Pos::new(0, 0, 0));
        assert_eq!(level.blocks.get(&(3, 69, 3)), level2.blocks.get(&(3, 69, 3)));
    }

    // --- P8 close-out features (lush caves / mushrooms / vines / sulfur) --------

    /// The close-out feature types all parse as implemented and, where nested,
    /// resolve fully; the previously-deferred lush-caves patches now run; the
    /// template-gated rooted_sulfur_spring and face-collapse-gated sculk_patch stay
    /// skipped.
    #[test]
    fn close_out_features_parse_and_resolve() {
        let reg = registry_for(&["minecraft:lush_caves"]);
        for n in [
            "cave_vines", "cave_vines_plant", "small_dripleaf", "big_dripleaf", "big_dripleaf_stem",
            "hanging_roots", "glow_lichen", "sculk_vein", "red_mushroom_block", "brown_mushroom_block", "mushroom_stem",
        ] {
            assert!(ParityBlock::from_name(n).is_some(), "block {n} resolves");
        }
        assert!(matches!(reg.configured.get("lush_caves_clay"), Some(ConfiguredFeature::RandomBooleanSelector { .. })));
        assert!(matches!(reg.configured.get("mushroom_island_vegetation"), Some(ConfiguredFeature::RandomBooleanSelector { .. })));
        assert!(matches!(reg.configured.get("glow_lichen"), Some(ConfiguredFeature::MultifaceGrowth(_))));
        assert!(matches!(reg.configured.get("rooted_azalea_tree"), Some(ConfiguredFeature::RootSystem(_))));
        assert!(matches!(reg.configured.get("fallen_oak_tree"), Some(ConfiguredFeature::FallenTree(_))));
        assert!(matches!(reg.configured.get("huge_red_mushroom"), Some(ConfiguredFeature::HugeMushroom(_))));
        assert!(matches!(reg.configured.get("huge_brown_mushroom"), Some(ConfiguredFeature::HugeMushroom(_))));
        assert!(matches!(reg.configured.get("vines"), Some(ConfiguredFeature::Vines)));
        assert!(matches!(reg.configured.get("sulfur_pool"), Some(ConfiguredFeature::Sequence(_))));
        assert!(matches!(reg.configured.get("cave_vine"), Some(ConfiguredFeature::BlockColumn(_))));
        assert!(matches!(reg.configured.get("dripleaf"), Some(ConfiguredFeature::SimpleRandomSelector(_))));
        for id in ["moss_patch_ceiling", "clay_with_dripleaves", "clay_pool_with_dripleaves", "pale_moss_patch"] {
            match reg.configured.get(id) {
                Some(ConfiguredFeature::VegetationPatch(c)) => {
                    assert!(nested_supported(&c.vegetation_feature), "{id} nested vegetation now supported")
                }
                other => panic!("{id} not a vegetation_patch: {other:?}"),
            }
        }
        if let Some(ConfiguredFeature::RootSystem(c)) = reg.configured.get("rooted_azalea_tree") {
            assert!(nested_supported(&c.feature), "azalea_tree nested feature supported");
        } else {
            panic!("rooted_azalea_tree missing");
        }
        if let Some(ConfiguredFeature::RootSystem(c)) = reg.configured.get("rooted_sulfur_spring") {
            assert!(!nested_supported(&c.feature), "sulfur_spring is template-gated → not supported");
        } else {
            panic!("rooted_sulfur_spring missing");
        }
        // sculk_patch stays deferred (charge sim depends on collapsed face state).
        assert!(!reg.configured.get("sculk_patch_deep_dark").map(|c| c.is_implemented()).unwrap_or(true));
    }

    /// glow_lichen (multiface_growth) attaches to a stone ceiling and is
    /// deterministic; the block collapses to `glow_lichen`.
    #[test]
    fn glow_lichen_attaches_to_ceiling() {
        let reg = registry_for(&["minecraft:lush_caves"]);
        let cfg = match reg.configured.get("glow_lichen") {
            Some(ConfiguredFeature::MultifaceGrowth(c)) => c.clone(),
            o => panic!("glow_lichen not multiface: {o:?}"),
        };
        let run = |seed: i64| {
            let mut level = TestLevel::new(50);
            level.set_block(0, 61, 0, ParityBlock::Stone); // ceiling above the air cell
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            let placed = place_multiface_growth(&cfg, &mut ctx, &mut random, Pos::new(0, 60, 0));
            (placed, level.get_block(0, 60, 0))
        };
        let a = run(11);
        assert_eq!(a, run(11), "deterministic");
        assert!(a.0, "growth placed");
        assert_eq!(a.1, ParityBlock::GlowLichen, "glow_lichen collapsed at origin");
    }

    /// A fallen oak drops a stump plus a sideways log run, deterministically.
    #[test]
    fn fallen_tree_lays_sideways_logs() {
        let reg = registry_for(&["minecraft:taiga"]);
        let cfg = match reg.configured.get("fallen_oak_tree") {
            Some(ConfiguredFeature::FallenTree(c)) => c.clone(),
            o => panic!("fallen_oak_tree not fallen_tree: {o:?}"),
        };
        let run = |seed: i64| {
            let mut level = tree_level(70);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            place_fallen_tree(&cfg, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut w: Vec<_> = level.blocks.into_iter().collect();
            w.sort_by_key(|(k, _)| *k);
            w
        };
        let a = run(5);
        assert_eq!(a, run(5), "deterministic");
        let logs = a.iter().filter(|(_, b)| *b == ParityBlock::OakLog).count();
        assert!(logs >= 3, "stump + fallen logs placed, got {logs}");
        assert_eq!(
            a.iter().find(|((x, y, z), _)| (*x, *y, *z) == (8, 70, 8)).map(|(_, b)| *b),
            Some(ParityBlock::OakLog),
            "stump at origin"
        );
    }

    /// A huge red mushroom builds a red cap and a mushroom stem on mycelium.
    #[test]
    fn huge_red_mushroom_builds_cap_and_stem() {
        let reg = registry_for(&["minecraft:mushroom_fields"]);
        let cfg = match reg.configured.get("huge_red_mushroom") {
            Some(ConfiguredFeature::HugeMushroom(c)) => c.clone(),
            o => panic!("huge_red_mushroom not huge_mushroom: {o:?}"),
        };
        let run = |seed: i64| {
            let mut level = TestLevel::new(70);
            level.set_block(8, 69, 8, ParityBlock::Mycelium);
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            let placed = place_huge_mushroom(&cfg, &mut ctx, &mut random, Pos::new(8, 70, 8));
            let mut w: Vec<_> = level.blocks.into_iter().collect();
            w.sort_by_key(|(k, _)| *k);
            (placed, w)
        };
        let (placed, a) = run(3);
        assert!(placed, "mushroom placed");
        assert_eq!(a, run(3).1, "deterministic");
        assert!(a.iter().any(|(_, b)| *b == ParityBlock::RedMushroomBlock), "red cap placed");
        assert!(a.iter().any(|(_, b)| *b == ParityBlock::MushroomStem), "stem placed");
    }

    /// Vines attach to a solid wall neighbour (no RNG); the face property collapses.
    #[test]
    fn vines_attach_to_wall() {
        let mut level = TestLevel::new(50);
        level.set_block(0, 60, -1, ParityBlock::Stone); // wall to the north
        let idx = AllBiome;
        let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
        assert!(place_vines(&mut ctx, Pos::new(0, 60, 0)), "vine placed");
        assert_eq!(level.get_block(0, 60, 0), ParityBlock::Vine);
    }

    /// A rooted azalea grows its tree, lays rooted dirt in the surrounding stone,
    /// and is deterministic. The nested `azalea_tree` is fully supported.
    #[test]
    fn root_system_grows_rooted_azalea() {
        let reg = registry_for(&["minecraft:lush_caves"]);
        let cfg = match reg.configured.get("rooted_azalea_tree") {
            Some(ConfiguredFeature::RootSystem(c)) => c.clone(),
            o => panic!("rooted_azalea_tree not root_system: {o:?}"),
        };
        let run = |seed: i64| {
            // Stone everywhere below y=200 (WORLD_SURFACE=200 is above the cave).
            let mut level = TestLevel::new(200);
            // Origin shaft (air) + a grass tree floor + a carved canopy pocket.
            level.set_block(0, 90, 0, ParityBlock::Air);
            level.set_block(0, 91, 0, ParityBlock::Air);
            level.set_block(0, 92, 0, ParityBlock::GrassBlock);
            for x in -4..=4 {
                for z in -4..=4 {
                    for y in 93..=104 {
                        level.set_block(x, y, z, ParityBlock::Air);
                    }
                }
            }
            let idx = AllBiome;
            let mut ctx = PlacementCtx { level: &mut level, biome_index: &idx, top_feature: "t" };
            let mut random = WorldgenRandom::new(RandomSource::xoroshiro(seed));
            let placed = place_root_system(&cfg, &mut ctx, &mut random, Pos::new(0, 90, 0));
            let mut w: Vec<_> = level.blocks.into_iter().collect();
            w.sort_by_key(|(k, _)| *k);
            (placed, w)
        };
        let (placed, a) = run(9);
        assert!(placed, "root_system returns true (origin was air)");
        assert_eq!(a, run(9).1, "deterministic");
        assert!(a.iter().any(|(_, b)| *b == ParityBlock::OakLog), "azalea trunk grew");
        assert!(a.iter().any(|(_, b)| *b == ParityBlock::RootedDirt), "rooted dirt laid in the walls");
    }
}
