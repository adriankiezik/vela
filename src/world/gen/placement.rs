//! P8 — placement modifiers, value providers, and block/rule predicates.
#![allow(dead_code)]
//!
//! Port of `net/minecraft/world/level/levelgen/placement/*` plus the value
//! providers (`util/valueproviders`, `heightproviders`), `VerticalAnchor`, the
//! `BlockPredicate` types, and the ore `RuleTest` types. A [`PlacedFeature`]
//! (see `features.rs`) threads a single origin through a chain of
//! [`PlacementModifier`]s as a **depth-first position stream** — matching
//! vanilla's lazy `Stream.flatMap` pull order, which is parity-critical because
//! each modifier draws from the shared decoration RNG as positions are pulled.
//!
//! Only the overworld-reachable modifiers are wired to exact behavior; the rest
//! parse into [`PlacementModifier::Unsupported`] (identity pass-through) and are
//! only ever reached by feature types this milestone defers (see `features.rs`
//! — deferred features are skipped entirely, so their placement never runs).

use serde_json::Value;

use super::density::ParityBlock;
use super::random::WorldgenRandom;

/// A block position (`BlockPos`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
    pub fn at_y(self, y: i32) -> Self {
        Self { y, ..self }
    }
    pub fn above(self, n: i32) -> Self {
        Self { y: self.y + n, ..self }
    }
    pub fn below(self) -> Self {
        Self { y: self.y - 1, ..self }
    }
    pub fn east(self) -> Self {
        Self { x: self.x + 1, ..self }
    }
    pub fn west(self) -> Self {
        Self { x: self.x - 1, ..self }
    }
    pub fn south(self) -> Self {
        Self { z: self.z + 1, ..self }
    }
    pub fn north(self) -> Self {
        Self { z: self.z - 1, ..self }
    }
    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self { x: self.x + dx, y: self.y + dy, z: self.z + dz }
    }
}

// ---------------------------------------------------------------------------
// The decoration level abstraction
// ---------------------------------------------------------------------------

/// The subset of `WorldGenLevel` the placement/feature code needs. Implemented
/// by the pipeline's `WorldGenRegion` (and by a test double).
pub trait DecorationLevel {
    fn get_block(&self, x: i32, y: i32, z: i32) -> ParityBlock;
    /// `setBlock` honoring the FEATURES write radius; returns whether it landed.
    fn set_block(&mut self, x: i32, y: i32, z: i32, state: ParityBlock) -> bool;
    /// `level.getHeight(type, x, z)` — the FEATURES-stage heightmaps.
    fn get_height(&self, heightmap: Heightmap, x: i32, z: i32) -> i32;
    /// `level.getBiome(pos)` as a parameter-list fill value (BiomeManager fuzzy
    /// zoom over the stored quart biomes).
    fn get_biome_fill(&self, x: i32, y: i32, z: i32) -> u16;
    fn min_y(&self) -> i32;
    fn gen_depth(&self) -> i32;
    fn sea_level(&self) -> i32;

    fn max_y(&self) -> i32 {
        self.min_y() + self.gen_depth()
    }
    fn is_outside_build_height(&self, y: i32) -> bool {
        y < self.min_y() || y >= self.max_y()
    }
}

/// `generator.getBiomeGenerationSettings(biome).hasFeature(placedFeature)` — the
/// biome→placed-feature membership `BiomeFilter` checks. Implemented by
/// `features::FeatureRegistry`.
pub trait BiomeFeatureIndex {
    fn biome_has_feature(&self, biome_fill: u16, placed_feature_id: &str) -> bool;
}

/// Everything a [`PlacementModifier`] / feature needs beyond the shared RNG.
pub struct PlacementCtx<'a> {
    pub level: &'a mut dyn DecorationLevel,
    pub biome_index: &'a dyn BiomeFeatureIndex,
    /// The placed-feature id currently being placed (for `BiomeFilter`).
    pub top_feature: &'a str,
}

impl PlacementCtx<'_> {
    fn get_height(&self, heightmap: Heightmap, x: i32, z: i32) -> i32 {
        self.level.get_height(heightmap, x, z)
    }
    fn min_y(&self) -> i32 {
        self.level.min_y()
    }
    fn gen_depth(&self) -> i32 {
        self.level.gen_depth()
    }
}

// ---------------------------------------------------------------------------
// Heightmap types
// ---------------------------------------------------------------------------

/// `Heightmap.Types` (only the worldgen-visible kinds).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heightmap {
    WorldSurfaceWg,
    OceanFloorWg,
    WorldSurface,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

impl Heightmap {
    pub fn from_str(name: &str) -> Option<Heightmap> {
        Some(match name {
            "WORLD_SURFACE_WG" => Heightmap::WorldSurfaceWg,
            "OCEAN_FLOOR_WG" => Heightmap::OceanFloorWg,
            "WORLD_SURFACE" => Heightmap::WorldSurface,
            "OCEAN_FLOOR" => Heightmap::OceanFloor,
            "MOTION_BLOCKING" => Heightmap::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Heightmap::MotionBlockingNoLeaves,
            _ => return None,
        })
    }

    /// `Heightmap.Types.isOpaque` over the parity alphabet — the predicate a
    /// column's height is the first-available above.
    pub fn matches(self, block: ParityBlock) -> bool {
        match self {
            Heightmap::WorldSurfaceWg | Heightmap::WorldSurface => !block.is_air(),
            Heightmap::OceanFloorWg | Heightmap::OceanFloor => block.blocks_motion(),
            Heightmap::MotionBlocking => block.blocks_motion() || block.is_fluid(),
            // `MOTION_BLOCKING_NO_LEAVES` is `MOTION_BLOCKING` minus `#leaves`.
            Heightmap::MotionBlockingNoLeaves => {
                !block.is_leaves() && (block.blocks_motion() || block.is_fluid())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VerticalAnchor
// ---------------------------------------------------------------------------

/// `VerticalAnchor` — resolved against `WorldGenerationContext`
/// (`min_gen_y = min_y`, `gen_depth = height`).
#[derive(Clone, Copy, Debug)]
pub enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl VerticalAnchor {
    pub fn parse(v: &Value) -> VerticalAnchor {
        if let Some(y) = v.get("absolute").and_then(Value::as_i64) {
            VerticalAnchor::Absolute(y as i32)
        } else if let Some(o) = v.get("above_bottom").and_then(Value::as_i64) {
            VerticalAnchor::AboveBottom(o as i32)
        } else if let Some(o) = v.get("below_top").and_then(Value::as_i64) {
            VerticalAnchor::BelowTop(o as i32)
        } else {
            panic!("bad vertical anchor: {v}")
        }
    }

    pub fn resolve_y(self, min_gen_y: i32, gen_depth: i32) -> i32 {
        match self {
            VerticalAnchor::Absolute(y) => y,
            VerticalAnchor::AboveBottom(o) => min_gen_y + o,
            VerticalAnchor::BelowTop(o) => gen_depth - 1 + min_gen_y - o,
        }
    }
}

// ---------------------------------------------------------------------------
// IntProvider
// ---------------------------------------------------------------------------

/// `Mth.randomBetweenInclusive` — `nextInt(max - min + 1) + min`.
fn random_between_inclusive(random: &mut WorldgenRandom, min: i32, max: i32) -> i32 {
    random.next_int_bounded(max - min + 1) + min
}

/// `IntProvider`. Fully exact for the variants overworld features reach
/// (`constant`, `uniform`); the biased/weighted/normal/trapezoid variants are
/// transcribed from their reference `sample` too, and only reached by feature
/// types deferred this milestone.
#[derive(Clone, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform { min: i32, max: i32 },
    BiasedToBottom { min: i32, max: i32 },
    Clamped { source: Box<IntProvider>, min: i32, max: i32 },
    ClampedNormal { mean: f32, deviation: f32, min: i32, max: i32 },
    WeightedList(Vec<(IntProvider, i32)>),
    Trapezoid { min: i32, max: i32, plateau: i32 },
}

impl IntProvider {
    pub fn parse(v: &Value) -> IntProvider {
        // A bare number is a `ConstantInt`.
        if let Some(n) = v.as_i64() {
            return IntProvider::Constant(n as i32);
        }
        let t = v.get("type").and_then(Value::as_str).unwrap_or("minecraft:constant");
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "constant" => IntProvider::Constant(v["value"].as_i64().unwrap_or(0) as i32),
            "uniform" => IntProvider::Uniform {
                min: v["min_inclusive"].as_i64().unwrap_or(0) as i32,
                max: v["max_inclusive"].as_i64().unwrap_or(0) as i32,
            },
            "biased_to_bottom" => IntProvider::BiasedToBottom {
                min: v["min_inclusive"].as_i64().unwrap_or(0) as i32,
                max: v["max_inclusive"].as_i64().unwrap_or(0) as i32,
            },
            "clamped" => IntProvider::Clamped {
                source: Box::new(IntProvider::parse(&v["source"])),
                min: v["min_inclusive"].as_i64().unwrap_or(0) as i32,
                max: v["max_inclusive"].as_i64().unwrap_or(0) as i32,
            },
            "clamped_normal" => IntProvider::ClampedNormal {
                mean: v["mean"].as_f64().unwrap_or(0.0) as f32,
                deviation: v["deviation"].as_f64().unwrap_or(0.0) as f32,
                min: v["min_inclusive"].as_i64().unwrap_or(0) as i32,
                max: v["max_inclusive"].as_i64().unwrap_or(0) as i32,
            },
            "trapezoid" => IntProvider::Trapezoid {
                min: v["min_inclusive"].as_i64().unwrap_or(0) as i32,
                max: v["max_inclusive"].as_i64().unwrap_or(0) as i32,
                plateau: v.get("plateau").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            "weighted_list" => {
                let empty = vec![]; let dist = v["distribution"].as_array().unwrap_or(&empty);
                IntProvider::WeightedList(
                    dist.iter()
                        .map(|e| {
                            (IntProvider::parse(&e["data"]), e["weight"].as_i64().unwrap_or(0) as i32)
                        })
                        .collect(),
                )
            }
            _ => IntProvider::Constant(0),
        }
    }

    pub fn sample(&self, random: &mut WorldgenRandom) -> i32 {
        match self {
            IntProvider::Constant(v) => *v,
            IntProvider::Uniform { min, max } => random_between_inclusive(random, *min, *max),
            // `BiasedToBottomInt`: `min + nextInt(nextInt(max-min+1)+1)`.
            IntProvider::BiasedToBottom { min, max } => {
                let inner = random.next_int_bounded(*max - *min + 1) + 1;
                *min + random.next_int_bounded(inner)
            }
            IntProvider::Clamped { source, min, max } => {
                source.sample(random).clamp(*min, *max)
            }
            // `ClampedNormalInt`: `clamp(round(normal), min, max)`.
            IntProvider::ClampedNormal { mean, deviation, min, max } => {
                let g = (*mean + random.next_gaussian() as f32 * *deviation).round() as i32;
                g.clamp(*min, *max)
            }
            IntProvider::WeightedList(entries) => {
                let total: i32 = entries.iter().map(|(_, w)| *w).sum();
                let mut roll = random.next_int_bounded(total);
                for (p, w) in entries {
                    roll -= *w;
                    if roll < 0 {
                        return p.sample(random);
                    }
                }
                entries.last().map(|(p, _)| p.sample(random)).unwrap_or(0)
            }
            // `TrapezoidInt.sample`.
            IntProvider::Trapezoid { min, max, plateau } => {
                let range = *max - *min;
                if *plateau >= range {
                    random_between_inclusive(random, *min, *max)
                } else {
                    let overhang = (range - *plateau) / 2;
                    let plateau_span = range - overhang;
                    *min + random.next_int_bounded(plateau_span + 1)
                        + random.next_int_bounded(overhang + 1)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HeightProvider
// ---------------------------------------------------------------------------

/// `HeightProvider`. Exact for the overworld-reachable kinds (`uniform`,
/// `trapezoid`, `very_biased_to_bottom`).
#[derive(Clone, Debug)]
pub enum HeightProvider {
    Constant(VerticalAnchor),
    Uniform { min: VerticalAnchor, max: VerticalAnchor },
    BiasedToBottom { min: VerticalAnchor, max: VerticalAnchor, inner: i32 },
    VeryBiasedToBottom { min: VerticalAnchor, max: VerticalAnchor, inner: i32 },
    Trapezoid { min: VerticalAnchor, max: VerticalAnchor, plateau: i32 },
}

impl HeightProvider {
    pub fn parse(v: &Value) -> HeightProvider {
        if let Some(y) = v.as_i64() {
            return HeightProvider::Constant(VerticalAnchor::Absolute(y as i32));
        }
        if v.get("absolute").is_some() || v.get("above_bottom").is_some() || v.get("below_top").is_some()
        {
            return HeightProvider::Constant(VerticalAnchor::parse(v));
        }
        let t = v.get("type").and_then(Value::as_str).unwrap_or("minecraft:uniform");
        let min = || VerticalAnchor::parse(&v["min_inclusive"]);
        let max = || VerticalAnchor::parse(&v["max_inclusive"]);
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "constant" => HeightProvider::Constant(VerticalAnchor::parse(&v["value"])),
            "uniform" => HeightProvider::Uniform { min: min(), max: max() },
            "biased_to_bottom" => HeightProvider::BiasedToBottom {
                min: min(),
                max: max(),
                inner: v.get("inner").and_then(Value::as_i64).unwrap_or(1) as i32,
            },
            "very_biased_to_bottom" => HeightProvider::VeryBiasedToBottom {
                min: min(),
                max: max(),
                inner: v.get("inner").and_then(Value::as_i64).unwrap_or(1) as i32,
            },
            "trapezoid" => HeightProvider::Trapezoid {
                min: min(),
                max: max(),
                plateau: v.get("plateau").and_then(Value::as_i64).unwrap_or(0) as i32,
            },
            _ => HeightProvider::Uniform { min: min(), max: max() },
        }
    }

    pub fn sample(&self, random: &mut WorldgenRandom, ctx: &PlacementCtx) -> i32 {
        let (min_gen_y, gen_depth) = (ctx.min_y(), ctx.gen_depth());
        let r = |a: VerticalAnchor| a.resolve_y(min_gen_y, gen_depth);
        match self {
            HeightProvider::Constant(a) => r(*a),
            HeightProvider::Uniform { min, max } => {
                let (lo, hi) = (r(*min), r(*max));
                if lo > hi {
                    lo
                } else {
                    random_between_inclusive(random, lo, hi)
                }
            }
            // `BiasedToBottomHeight.sample`.
            HeightProvider::BiasedToBottom { min, max, inner } => {
                let (lo, hi) = (r(*min), r(*max));
                if hi - lo - *inner + 1 <= 0 {
                    lo
                } else {
                    let j = random.next_int_bounded(hi - lo - *inner + 1);
                    let k = random.next_int_bounded(j + *inner);
                    lo + k
                }
            }
            // `VeryBiasedToBottomHeight.sample`.
            HeightProvider::VeryBiasedToBottom { min, max, inner } => {
                let (lo, hi) = (r(*min), r(*max));
                if hi - lo - *inner + 1 <= 0 {
                    lo
                } else {
                    let j = random.next_int_between(lo + *inner, hi);
                    let k = random.next_int_between(lo, j - 1);
                    let l = random.next_int_between(lo, k - 1 + *inner);
                    l
                }
            }
            // `TrapezoidHeight.sample`.
            HeightProvider::Trapezoid { min, max, plateau } => {
                let (lo, hi) = (r(*min), r(*max));
                let range = hi - lo;
                if *plateau >= range {
                    random_between_inclusive(random, lo, hi)
                } else {
                    let overhang = (range - *plateau) / 2;
                    let plateau_span = range - overhang;
                    lo + random.next_int_bounded(plateau_span + 1)
                        + random.next_int_bounded(overhang + 1)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RuleTest (ore targets)
// ---------------------------------------------------------------------------

/// `RuleTest` — the ore-target predicates. Overworld ores use `tag_match`
/// (against the three ore-replaceable block tags) and `block_match`.
#[derive(Clone, Debug)]
pub enum RuleTest {
    AlwaysTrue,
    BlockMatch(ParityBlock),
    /// A vanilla block tag, resolved to the parity-alphabet members.
    TagMatch(BlockTag),
    RandomBlockMatch { block: ParityBlock, probability: f32 },
    Unsupported,
}

impl RuleTest {
    pub fn parse(v: &Value) -> RuleTest {
        let t = v.get("predicate_type").and_then(Value::as_str).unwrap_or("");
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "always_true" => RuleTest::AlwaysTrue,
            "block_match" => v["block"]
                .as_str()
                .and_then(ParityBlock::from_name)
                .map(RuleTest::BlockMatch)
                .unwrap_or(RuleTest::Unsupported),
            "tag_match" => match v["tag"].as_str().and_then(BlockTag::from_id) {
                Some(tag) => RuleTest::TagMatch(tag),
                None => RuleTest::Unsupported,
            },
            "random_block_match" => match v["block"].as_str().and_then(ParityBlock::from_name) {
                Some(block) => RuleTest::RandomBlockMatch {
                    block,
                    probability: v["probability"].as_f64().unwrap_or(0.0) as f32,
                },
                None => RuleTest::Unsupported,
            },
            _ => RuleTest::Unsupported,
        }
    }

    pub fn test(&self, block: ParityBlock, random: &mut WorldgenRandom) -> bool {
        match self {
            RuleTest::AlwaysTrue => true,
            RuleTest::BlockMatch(b) => block == *b,
            RuleTest::TagMatch(tag) => tag.contains(block),
            RuleTest::RandomBlockMatch { block: b, probability } => {
                block == *b && random.next_float() < *probability
            }
            RuleTest::Unsupported => false,
        }
    }
}

/// The handful of vanilla block tags the overworld ore rules reference. Members
/// transcribed from `data/minecraft/tags/block/*` (stable vanilla data).
#[derive(Clone, Copy, Debug)]
pub enum BlockTag {
    StoneOreReplaceables,
    DeepslateOreReplaceables,
    BaseStoneOverworld,
    /// `#minecraft:logs` — over the parity alphabet, the plain overworld logs.
    Logs,
    /// `#minecraft:leaves`.
    Leaves,
    /// `#minecraft:dirt`.
    Dirt,
    /// `#minecraft:replaceable_by_trees` (leaves ∪ vegetation ∪ water; only the
    /// parity-alphabet members: leaves and water — air is handled separately by
    /// `TreeFeature.validTreePos`).
    ReplaceableByTrees,
    /// `#minecraft:cannot_replace_below_tree_trunk`.
    CannotReplaceBelowTreeTrunk,
    /// `#minecraft:supports_vegetation` (= `#substrate_overworld ∪ farmland`) —
    /// the sapling floor used by `would_survive`.
    SupportsVegetation,
    /// `#minecraft:supports_mangrove_propagule` (= `#supports_vegetation ∪ clay`).
    SupportsMangrovePropagule,
    /// `#minecraft:beneath_tree_podzol_replaceable` (= `#substrate_overworld`) —
    /// the alter_ground decorator's podzol-replaceable floor for mega spruce/pine.
    BeneathTreePodzolReplaceable,
    /// `#minecraft:mangrove_logs_can_grow_through` — the upwards-branching trunk
    /// placer's `validTreePos` override.
    MangroveLogsCanGrowThrough,
    /// `#minecraft:mangrove_roots_can_grow_through` — the mangrove root placer's
    /// `canPlaceRoot` override.
    MangroveRootsCanGrowThrough,
    /// `#minecraft:air` — not a vanilla tag, but lets `matching_block_tag`/vegetation
    /// predicates resolve air instead of silently failing.
    Air,
    /// `#minecraft:features_cannot_replace` — protected blocks the lake feature
    /// must not overwrite (only `bedrock` is in the parity alphabet).
    FeaturesCannotReplace,
    /// `#minecraft:lava_pool_stone_cannot_replace` (= `#features_cannot_replace ∪
    /// #leaves ∪ #logs`) — the lava-lake barrier cannot replace these.
    LavaPoolStoneCannotReplace,
}

impl BlockTag {
    pub fn from_id(id: &str) -> Option<BlockTag> {
        Some(match id.strip_prefix("minecraft:").unwrap_or(id) {
            "stone_ore_replaceables" => BlockTag::StoneOreReplaceables,
            "deepslate_ore_replaceables" => BlockTag::DeepslateOreReplaceables,
            "base_stone_overworld" => BlockTag::BaseStoneOverworld,
            "logs" => BlockTag::Logs,
            "leaves" => BlockTag::Leaves,
            "dirt" => BlockTag::Dirt,
            "replaceable_by_trees" => BlockTag::ReplaceableByTrees,
            "cannot_replace_below_tree_trunk" => BlockTag::CannotReplaceBelowTreeTrunk,
            "supports_vegetation" => BlockTag::SupportsVegetation,
            "supports_mangrove_propagule" => BlockTag::SupportsMangrovePropagule,
            "beneath_tree_podzol_replaceable" => BlockTag::BeneathTreePodzolReplaceable,
            "mangrove_logs_can_grow_through" => BlockTag::MangroveLogsCanGrowThrough,
            "mangrove_roots_can_grow_through" => BlockTag::MangroveRootsCanGrowThrough,
            "air" => BlockTag::Air,
            "features_cannot_replace" => BlockTag::FeaturesCannotReplace,
            "lava_pool_stone_cannot_replace" => BlockTag::LavaPoolStoneCannotReplace,
            _ => return None,
        })
    }

    pub fn contains(self, b: ParityBlock) -> bool {
        use ParityBlock::*;
        match self {
            BlockTag::StoneOreReplaceables => matches!(b, Stone | Granite | Diorite | Andesite),
            BlockTag::DeepslateOreReplaceables => matches!(b, Deepslate | Tuff),
            BlockTag::BaseStoneOverworld => {
                matches!(b, Stone | Granite | Diorite | Andesite | Tuff | Deepslate)
            }
            BlockTag::Logs => matches!(
                b,
                OakLog | BirchLog | SpruceLog | DarkOakLog | JungleLog | AcaciaLog | CherryLog | MangroveLog
            ),
            BlockTag::Leaves => b.is_leaves(),
            BlockTag::Dirt => matches!(b, Dirt | CoarseDirt | RootedDirt),
            BlockTag::ReplaceableByTrees => b.is_leaves() || matches!(b, Water),
            // `#dirt ∪ #mud ∪ #moss_blocks ∪ podzol` over the parity alphabet.
            BlockTag::CannotReplaceBelowTreeTrunk => {
                matches!(b, Dirt | CoarseDirt | RootedDirt | Mud | MuddyMangroveRoots | Podzol)
            }
            // `#supports_vegetation` = `#substrate_overworld ∪ farmland`
            // (farmland is not in the alphabet).
            BlockTag::SupportsVegetation => matches!(
                b,
                Dirt | CoarseDirt | RootedDirt | Mud | MuddyMangroveRoots | GrassBlock | Podzol | Mycelium
                    | MossBlock | PaleMossBlock
            ),
            // `#supports_mangrove_propagule` = `#supports_vegetation ∪ clay`.
            BlockTag::SupportsMangrovePropagule => matches!(
                b,
                Dirt | CoarseDirt | RootedDirt | Mud | MuddyMangroveRoots | GrassBlock | Podzol | Mycelium
                    | MossBlock | PaleMossBlock | Clay
            ),
            // `#substrate_overworld` = `#dirt ∪ #mud ∪ #moss_blocks ∪ #grass_blocks`.
            // (`#moss_blocks` = moss_block/pale_moss_block, not in the alphabet.)
            BlockTag::BeneathTreePodzolReplaceable => matches!(
                b,
                Dirt | CoarseDirt | RootedDirt | Mud | MuddyMangroveRoots | GrassBlock | Podzol | Mycelium
            ),
            BlockTag::MangroveLogsCanGrowThrough => matches!(
                b,
                Mud | MuddyMangroveRoots | MangroveRoots | MangroveLeaves | MangroveLog | MangrovePropagule | MossCarpet | Vine
            ),
            BlockTag::MangroveRootsCanGrowThrough => matches!(
                b,
                Mud | MuddyMangroveRoots | MangroveRoots | MossCarpet | Vine | MangrovePropagule | Snow
            ),
            BlockTag::Air => b.is_air(),
            BlockTag::FeaturesCannotReplace => matches!(b, Bedrock),
            BlockTag::LavaPoolStoneCannotReplace => {
                matches!(b, Bedrock) || b.is_leaves() || BlockTag::Logs.contains(b)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BlockPredicate
// ---------------------------------------------------------------------------

/// `BlockPredicate`. Exact for the predicates the implemented features reach
/// (`matching_fluids`, `matching_blocks`, `true`); the rest are transcribed
/// too where cheap and otherwise conservative (only reached by deferred
/// features, which never run their placement).
#[derive(Clone, Debug)]
pub enum BlockPredicate {
    True,
    MatchingBlocks { blocks: Vec<ParityBlock>, offset: (i32, i32, i32) },
    MatchingFluids { fluids: Vec<ParityBlock>, offset: (i32, i32, i32) },
    MatchingBlockTag { tag: Option<BlockTag>, offset: (i32, i32, i32) },
    Replaceable { offset: (i32, i32, i32) },
    Solid { offset: (i32, i32, i32) },
    InsideWorldBounds { offset: (i32, i32, i32) },
    /// `would_survive` — `state.canSurvive(pos)`. For the plant states the tree
    /// placed features use this reduces to "the block below is in the plant's
    /// `mayPlaceOn` tag": saplings → `#supports_vegetation`, mangrove propagule
    /// (non-hanging) → `#supports_mangrove_propagule`. `support == None` is a
    /// conservative `false` for any state not modeled, matching the deferred-only
    /// note on the other unsupported predicates.
    WouldSurvive { support: Option<BlockTag>, offset: (i32, i32, i32) },
    Not(Box<BlockPredicate>),
    AllOf(Vec<BlockPredicate>),
    AnyOf(Vec<BlockPredicate>),
    /// `would_survive` needs full block placement logic (leaves/plants) — only
    /// used by deferred vegetation features, so it is left conservative.
    Unsupported,
}

fn parse_offset(v: &Value) -> (i32, i32, i32) {
    match v.get("offset").and_then(Value::as_array) {
        Some(a) if a.len() == 3 => {
            (a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32, a[2].as_i64().unwrap_or(0) as i32)
        }
        _ => (0, 0, 0),
    }
}

fn parse_block_list(v: &Value) -> Vec<ParityBlock> {
    match v {
        Value::String(s) => ParityBlock::from_name(s).into_iter().collect(),
        Value::Array(a) => a.iter().filter_map(|e| e.as_str().and_then(ParityBlock::from_name)).collect(),
        _ => Vec::new(),
    }
}

impl BlockPredicate {
    pub fn parse(v: &Value) -> BlockPredicate {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        let off = parse_offset(v);
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "true" => BlockPredicate::True,
            "matching_blocks" => BlockPredicate::MatchingBlocks {
                blocks: parse_block_list(&v["blocks"]),
                offset: off,
            },
            "matching_fluids" => BlockPredicate::MatchingFluids {
                fluids: parse_block_list(&v["fluids"]),
                offset: off,
            },
            "matching_block_tag" => BlockPredicate::MatchingBlockTag {
                tag: v["tag"].as_str().and_then(BlockTag::from_id),
                offset: off,
            },
            "replaceable" => BlockPredicate::Replaceable { offset: off },
            "solid" => BlockPredicate::Solid { offset: off },
            "inside_world_bounds" => BlockPredicate::InsideWorldBounds { offset: off },
            "would_survive" => {
                let name = v["state"]["Name"].as_str().unwrap_or("");
                let name = name.strip_prefix("minecraft:").unwrap_or(name);
                let support = if name.ends_with("_sapling") {
                    Some(BlockTag::SupportsVegetation)
                } else if name == "mangrove_propagule" {
                    // The placed state has `hanging=false` → `SaplingBlock.canSurvive`
                    // → `mayPlaceOn(below) = #supports_mangrove_propagule`.
                    Some(BlockTag::SupportsMangrovePropagule)
                } else {
                    None
                };
                BlockPredicate::WouldSurvive { support, offset: off }
            }
            "not" => BlockPredicate::Not(Box::new(BlockPredicate::parse(&v["predicate"]))),
            "all_of" => BlockPredicate::AllOf(
                v["predicates"].as_array().unwrap_or(&vec![]).iter().map(BlockPredicate::parse).collect(),
            ),
            "any_of" => BlockPredicate::AnyOf(
                v["predicates"].as_array().unwrap_or(&vec![]).iter().map(BlockPredicate::parse).collect(),
            ),
            _ => BlockPredicate::Unsupported,
        }
    }

    pub fn test(&self, level: &dyn DecorationLevel, pos: Pos) -> bool {
        let at = |o: (i32, i32, i32)| level.get_block(pos.x + o.0, pos.y + o.1, pos.z + o.2);
        match self {
            BlockPredicate::True => true,
            BlockPredicate::MatchingBlocks { blocks, offset } => blocks.contains(&at(*offset)),
            BlockPredicate::MatchingFluids { fluids, offset } => fluids.contains(&at(*offset)),
            BlockPredicate::MatchingBlockTag { tag, offset } => {
                tag.map(|t| t.contains(at(*offset))).unwrap_or(false)
            }
            // `replaceable`: air or fluid over the parity alphabet.
            BlockPredicate::Replaceable { offset } => {
                let b = at(*offset);
                b.is_air() || b.is_fluid()
            }
            BlockPredicate::Solid { offset } => at(*offset).blocks_motion(),
            BlockPredicate::InsideWorldBounds { offset } => {
                !level.is_outside_build_height(pos.y + offset.1)
            }
            BlockPredicate::WouldSurvive { support, offset } => match support {
                None => false,
                Some(tag) => {
                    let bx = pos.x + offset.0;
                    let by = pos.y + offset.1;
                    let bz = pos.z + offset.2;
                    tag.contains(level.get_block(bx, by - 1, bz))
                }
            },
            BlockPredicate::Not(p) => !p.test(level, pos),
            BlockPredicate::AllOf(ps) => ps.iter().all(|p| p.test(level, pos)),
            BlockPredicate::AnyOf(ps) => ps.iter().any(|p| p.test(level, pos)),
            BlockPredicate::Unsupported => false,
        }
    }
}

// ---------------------------------------------------------------------------
// PlacementModifier
// ---------------------------------------------------------------------------

/// `Direction` (vertical only, for `environment_scan`).
#[derive(Clone, Copy, Debug)]
pub enum VDir {
    Up,
    Down,
}

/// `PlacementModifier`. `get_positions` draws all its RNG eagerly and returns a
/// small position list; the depth-first driver in `features.rs` pulls one
/// position through the remaining chain before the next, reproducing vanilla's
/// lazy `flatMap` RNG order exactly.
#[derive(Clone, Debug)]
pub enum PlacementModifier {
    Count(IntProvider),
    Rarity(i32),
    InSquare,
    HeightRange(HeightProvider),
    Heightmap(Heightmap),
    Biome,
    BlockPredicateFilter(BlockPredicate),
    RandomOffset { xz: IntProvider, y: IntProvider },
    SurfaceWaterDepth(i32),
    SurfaceRelativeThreshold { heightmap: Heightmap, min: i32, max: i32 },
    CountOnEveryLayer(IntProvider),
    EnvironmentScan { dir: VDir, target: BlockPredicate, allowed: BlockPredicate, max_steps: i32 },
    NoiseThresholdCount { noise_level: f64, below: i32, above: i32 },
    NoiseBasedCount { ratio: i32, factor: f64, offset: f64 },
    /// A modifier not (yet) wired to exact behavior. Acts as identity; only
    /// reached by deferred features (whose placement never runs).
    Unsupported(String),
}

impl PlacementModifier {
    pub fn parse(v: &Value) -> PlacementModifier {
        let t = v.get("type").and_then(Value::as_str).unwrap_or("");
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "count" => PlacementModifier::Count(IntProvider::parse(&v["count"])),
            "rarity_filter" => PlacementModifier::Rarity(v["chance"].as_i64().unwrap_or(0) as i32),
            "in_square" => PlacementModifier::InSquare,
            "height_range" => PlacementModifier::HeightRange(HeightProvider::parse(&v["height"])),
            "heightmap" => PlacementModifier::Heightmap(
                Heightmap::from_str(v["heightmap"].as_str().unwrap()).expect("heightmap"),
            ),
            "biome" => PlacementModifier::Biome,
            "block_predicate_filter" => {
                PlacementModifier::BlockPredicateFilter(BlockPredicate::parse(&v["predicate"]))
            }
            "random_offset" => PlacementModifier::RandomOffset {
                xz: IntProvider::parse(&v["xz_spread"]),
                y: IntProvider::parse(&v["y_spread"]),
            },
            "surface_water_depth_filter" => {
                PlacementModifier::SurfaceWaterDepth(v["max_water_depth"].as_i64().unwrap_or(0) as i32)
            }
            "surface_relative_threshold_filter" => PlacementModifier::SurfaceRelativeThreshold {
                heightmap: Heightmap::from_str(v["heightmap"].as_str().unwrap()).expect("heightmap"),
                min: v.get("min_inclusive").and_then(Value::as_i64).unwrap_or(i32::MIN as i64) as i32,
                max: v.get("max_inclusive").and_then(Value::as_i64).unwrap_or(i32::MAX as i64) as i32,
            },
            "count_on_every_layer" => {
                PlacementModifier::CountOnEveryLayer(IntProvider::parse(&v["count"]))
            }
            "environment_scan" => PlacementModifier::EnvironmentScan {
                dir: if v["direction_of_search"].as_str() == Some("up") { VDir::Up } else { VDir::Down },
                target: BlockPredicate::parse(&v["target_condition"]),
                allowed: v
                    .get("allowed_search_condition")
                    .map(BlockPredicate::parse)
                    .unwrap_or(BlockPredicate::True),
                max_steps: v["max_steps"].as_i64().unwrap_or(0) as i32,
            },
            "noise_threshold_count" => PlacementModifier::NoiseThresholdCount {
                noise_level: v["noise_level"].as_f64().unwrap_or(0.0),
                below: v["below_noise"].as_i64().unwrap_or(0) as i32,
                above: v["above_noise"].as_i64().unwrap_or(0) as i32,
            },
            "noise_based_count" => PlacementModifier::NoiseBasedCount {
                ratio: v["noise_to_count_ratio"].as_i64().unwrap_or(0) as i32,
                factor: v["noise_factor"].as_f64().unwrap_or(1.0),
                offset: v.get("noise_offset").and_then(Value::as_f64).unwrap_or(0.0),
            },
            other => PlacementModifier::Unsupported(other.to_owned()),
        }
    }

    /// Whether this modifier is wired to exact behavior (used to decide if a
    /// feature is safe to place — see `features.rs`).
    pub fn is_supported(&self) -> bool {
        !matches!(self, PlacementModifier::Unsupported(_))
    }

    pub fn get_positions(
        &self,
        ctx: &mut PlacementCtx,
        random: &mut WorldgenRandom,
        origin: Pos,
    ) -> Vec<Pos> {
        match self {
            PlacementModifier::Count(c) => {
                let n = c.sample(random);
                vec![origin; n.max(0) as usize]
            }
            PlacementModifier::Rarity(chance) => {
                if random.next_float() < 1.0 / *chance as f32 {
                    vec![origin]
                } else {
                    vec![]
                }
            }
            PlacementModifier::InSquare => {
                let x = random.next_int_bounded(16) + origin.x;
                let z = random.next_int_bounded(16) + origin.z;
                vec![Pos::new(x, origin.y, z)]
            }
            PlacementModifier::HeightRange(h) => {
                let y = h.sample(random, ctx);
                vec![origin.at_y(y)]
            }
            PlacementModifier::Heightmap(hm) => {
                let y = ctx.get_height(*hm, origin.x, origin.z);
                if y > ctx.min_y() {
                    vec![Pos::new(origin.x, y, origin.z)]
                } else {
                    vec![]
                }
            }
            PlacementModifier::Biome => {
                let fill = ctx.level.get_biome_fill(origin.x, origin.y, origin.z);
                if ctx.biome_index.biome_has_feature(fill, ctx.top_feature) {
                    vec![origin]
                } else {
                    vec![]
                }
            }
            PlacementModifier::BlockPredicateFilter(pred) => {
                if pred.test(ctx.level, origin) {
                    vec![origin]
                } else {
                    vec![]
                }
            }
            PlacementModifier::RandomOffset { xz, y } => {
                let sx = origin.x + xz.sample(random);
                let sy = origin.y + y.sample(random);
                let sz = origin.z + xz.sample(random);
                vec![Pos::new(sx, sy, sz)]
            }
            PlacementModifier::SurfaceWaterDepth(max) => {
                let floor = ctx.get_height(Heightmap::OceanFloor, origin.x, origin.z);
                let surf = ctx.get_height(Heightmap::WorldSurface, origin.x, origin.z);
                if surf - floor <= *max {
                    vec![origin]
                } else {
                    vec![]
                }
            }
            PlacementModifier::SurfaceRelativeThreshold { heightmap, min, max } => {
                let surface = ctx.get_height(*heightmap, origin.x, origin.z) as i64;
                let lo = surface + *min as i64;
                let hi = surface + *max as i64;
                if lo <= origin.y as i64 && origin.y as i64 <= hi {
                    vec![origin]
                } else {
                    vec![]
                }
            }
            PlacementModifier::CountOnEveryLayer(count) => {
                let mut out = Vec::new();
                let mut layer = 0;
                loop {
                    let mut found_any = false;
                    let n = count.sample(random);
                    for _ in 0..n {
                        let x = random.next_int_bounded(16) + origin.x;
                        let z = random.next_int_bounded(16) + origin.z;
                        let start_y = ctx.get_height(Heightmap::MotionBlocking, x, z);
                        let y = find_on_ground_y(ctx, x, start_y, z, layer);
                        if y != i32::MAX {
                            out.push(Pos::new(x, y, z));
                            found_any = true;
                        }
                    }
                    layer += 1;
                    if !found_any {
                        break;
                    }
                }
                out
            }
            PlacementModifier::EnvironmentScan { dir, target, allowed, max_steps } => {
                let mut pos = origin;
                if !allowed.test(ctx.level, pos) {
                    return vec![];
                }
                let dy = match dir {
                    VDir::Up => 1,
                    VDir::Down => -1,
                };
                for _ in 0..*max_steps {
                    if target.test(ctx.level, pos) {
                        return vec![pos];
                    }
                    pos = Pos::new(pos.x, pos.y + dy, pos.z);
                    if ctx.level.is_outside_build_height(pos.y) {
                        return vec![];
                    }
                    if !allowed.test(ctx.level, pos) {
                        break;
                    }
                }
                if target.test(ctx.level, pos) {
                    vec![pos]
                } else {
                    vec![]
                }
            }
            PlacementModifier::NoiseThresholdCount { noise_level, below, above } => {
                let noise = super::features::biome_info_noise(origin.x as f64 / 200.0, origin.z as f64 / 200.0);
                let n = if noise < *noise_level { *below } else { *above };
                vec![origin; n.max(0) as usize]
            }
            PlacementModifier::NoiseBasedCount { ratio, factor, offset } => {
                let noise = super::features::biome_info_noise(origin.x as f64 / *factor, origin.z as f64 / *factor);
                let n = ((noise + *offset) * *ratio as f64).ceil() as i32;
                vec![origin; n.max(0) as usize]
            }
            PlacementModifier::Unsupported(_) => vec![origin],
        }
    }
}

/// `CountOnEveryLayerPlacement.findOnGroundYPosition`.
fn find_on_ground_y(ctx: &PlacementCtx, x: i32, y_start: i32, z: i32, layer_to_place_on: i32) -> i32 {
    let is_empty = |b: ParityBlock| b.is_air() || b == ParityBlock::Water || b == ParityBlock::Lava;
    let mut current_layer = 0;
    let mut current = ctx.level.get_block(x, y_start, z);
    let mut y = y_start;
    while y >= ctx.min_y() + 1 {
        let below = ctx.level.get_block(x, y - 1, z);
        if !is_empty(below) && is_empty(current) && below != ParityBlock::Bedrock {
            if current_layer == layer_to_place_on {
                return y;
            }
            current_layer += 1;
        }
        current = below;
        y -= 1;
    }
    i32::MAX
}
