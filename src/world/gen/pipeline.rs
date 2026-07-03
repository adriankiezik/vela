//! P6 — the staged chunk-generation pipeline: vanilla's chunk statuses, the
//! dependency pyramid, proto-chunks, a `WorldGenRegion` view, and a scheduler
//! that generates a chunk's dependencies before advancing it.
//!
//! Reference: `world/level/chunk/status/` (`ChunkStatus`, `ChunkDependencies`,
//! `ChunkStep`, `ChunkPyramid`, `ChunkStatusTasks`) and
//! `server/level/WorldGenRegion`. Vanilla generates through 12 statuses
//! (empty → … → full); each status's step declares per-radius requirements on
//! neighbor chunks (`addRequirement`) and a block-state write radius. The
//! pyramid's *accumulated* dependencies answer "how far out, and at what
//! status, must neighbors exist for this chunk to reach status S" — e.g. FULL
//! accumulates `[SPAWN, INITIALIZE_LIGHT, CARVERS, BIOMES, STRUCTURE_STARTS×8]`.
//!
//! The stage tasks route to the P2–P5 engines: BIOMES fills the per-section
//! quart biomes (`fillBiomesFromNoise`), NOISE runs `doFill`
//! (aquifers/ore veins included), SURFACE applies the surface rules over the
//! biomes *stored* in the 3×3 neighborhood (exactly vanilla, which reads
//! neighbor chunks at ≥ BIOMES through the region rather than resampling).
//! CARVERS runs `applyCarvers` (P7, `super::carvers`) over the center chunk.
//! STRUCTURE_STARTS / STRUCTURE_REFERENCES (P9) and FEATURES (P8) are
//! vanilla-shaped no-ops for now — for terrain output that is *exact* wherever
//! no structure/feature would touch, and the pipeline is the architecture those
//! layers plug into. Light/spawn statuses are also no-ops: Vela computes light
//! at encode time and spawns mobs in the sim.
//!
//! Vanilla's LOADING_PYRAMID (chunks read from disk re-run only light tasks)
//! collapses here to: only `minecraft:full` chunks are loaded from disk;
//! anything less regenerates — output-identical while generation stays
//! deterministic and cross-chunk writes don't exist (they arrive with P8,
//! which is when intermediate-status persistence starts to matter).

// The pipeline is the P6 architecture layer: its consumers are the golden
// tests today and the carver/feature/structure stages (P7–P9) plus the live
// chunk path next. Until those land, most of the API has no non-test caller.
// Drop this once the live generator drives the pipeline.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tracing::warn;

use super::climate::MultiNoiseBiomeSource;
use super::density::{FilledChunk, NoiseChunk, ParityBlock};
use super::features::{self, FeatureRegistry};
use super::placement::{DecorationLevel, Heightmap};
use super::surface_rules::{obfuscate_seed, zoomed_quart, BakedBiomes, SurfacedGenerator};

// ---------------------------------------------------------------------------
// ChunkStatus
// ---------------------------------------------------------------------------

/// `ChunkStatus` — the 12 generation statuses, in pyramid order. The
/// discriminant is vanilla's `getIndex()`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum ChunkStatus {
    Empty = 0,
    StructureStarts = 1,
    StructureReferences = 2,
    Biomes = 3,
    Noise = 4,
    Surface = 5,
    Carvers = 6,
    Features = 7,
    InitializeLight = 8,
    Light = 9,
    Spawn = 10,
    Full = 11,
}

impl ChunkStatus {
    pub const ALL: [ChunkStatus; 12] = [
        ChunkStatus::Empty,
        ChunkStatus::StructureStarts,
        ChunkStatus::StructureReferences,
        ChunkStatus::Biomes,
        ChunkStatus::Noise,
        ChunkStatus::Surface,
        ChunkStatus::Carvers,
        ChunkStatus::Features,
        ChunkStatus::InitializeLight,
        ChunkStatus::Light,
        ChunkStatus::Spawn,
        ChunkStatus::Full,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The registry id persisted in chunk NBT (`Status`).
    pub fn name(self) -> &'static str {
        match self {
            ChunkStatus::Empty => "minecraft:empty",
            ChunkStatus::StructureStarts => "minecraft:structure_starts",
            ChunkStatus::StructureReferences => "minecraft:structure_references",
            ChunkStatus::Biomes => "minecraft:biomes",
            ChunkStatus::Noise => "minecraft:noise",
            ChunkStatus::Surface => "minecraft:surface",
            ChunkStatus::Carvers => "minecraft:carvers",
            ChunkStatus::Features => "minecraft:features",
            ChunkStatus::InitializeLight => "minecraft:initialize_light",
            ChunkStatus::Light => "minecraft:light",
            ChunkStatus::Spawn => "minecraft:spawn",
            ChunkStatus::Full => "minecraft:full",
        }
    }

    /// Parse a persisted `Status` string (with or without the `minecraft:`
    /// namespace), mirroring `ChunkStatus.byName`.
    pub fn from_name(name: &str) -> Option<ChunkStatus> {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        Self::ALL
            .into_iter()
            .find(|s| s.name().strip_prefix("minecraft:") == Some(bare))
    }

    pub fn is_or_after(self, other: ChunkStatus) -> bool {
        self >= other
    }
}

// ---------------------------------------------------------------------------
// ChunkDependencies / ChunkStep / ChunkPyramid
// ---------------------------------------------------------------------------

/// `ChunkDependencies` — for a generating step, the status required of a
/// neighbor chunk at each chessboard distance (`dependency_by_radius[d]`),
/// plus the inverse lookup "out to what radius is status S required".
#[derive(Debug, PartialEq, Eq)]
pub struct ChunkDependencies {
    dependency_by_radius: Vec<ChunkStatus>,
    radius_by_dependency: Vec<i32>,
}

impl ChunkDependencies {
    fn new(dependency_by_radius: Vec<ChunkStatus>) -> Self {
        let size = dependency_by_radius.first().map_or(0, |s| s.index() + 1);
        let mut radius_by_dependency = vec![0i32; size];
        for (radius, dependency) in dependency_by_radius.iter().enumerate() {
            for entry in radius_by_dependency.iter_mut().take(dependency.index() + 1) {
                *entry = radius as i32;
            }
        }
        Self { dependency_by_radius, radius_by_dependency }
    }

    pub fn size(&self) -> i32 {
        self.dependency_by_radius.len() as i32
    }

    /// The furthest chessboard distance at which `status` is required.
    /// Panics (like vanilla) when `status` is outside the dependency range.
    pub fn radius_of(&self, status: ChunkStatus) -> i32 {
        self.radius_by_dependency
            .get(status.index())
            .copied()
            .unwrap_or_else(|| {
                panic!("requesting a ChunkStatus({status:?}) outside of dependency range")
            })
    }

    pub fn radius(&self) -> i32 {
        (self.dependency_by_radius.len() as i32 - 1).max(0)
    }

    pub fn get(&self, distance: i32) -> ChunkStatus {
        self.dependency_by_radius[distance as usize]
    }

    #[cfg(test)]
    fn as_slice(&self) -> &[ChunkStatus] {
        &self.dependency_by_radius
    }
}

/// `ChunkStep` — one pyramid step: the status it produces, its direct and
/// accumulated neighbor requirements, and how far block writes may reach.
pub struct ChunkStep {
    pub target_status: ChunkStatus,
    pub direct_dependencies: ChunkDependencies,
    pub accumulated_dependencies: ChunkDependencies,
    /// `blockStateWriteRadius` — `-1` for steps that place no blocks.
    pub block_state_write_radius: i32,
}

impl ChunkStep {
    /// `getAccumulatedRadiusOf` — 0 for the step's own status.
    pub fn accumulated_radius_of(&self, status: ChunkStatus) -> i32 {
        if status == self.target_status {
            0
        } else {
            self.accumulated_dependencies.radius_of(status)
        }
    }
}

/// `ChunkStep.Builder`, ported exactly: `add_requirement` widens the
/// per-radius array keeping the max status at each distance, and the
/// accumulated dependencies merge the parent step's accumulation shifted out
/// by the parent's own radius in this step.
struct StepBuilder {
    status: ChunkStatus,
    parent: Option<usize>,
    direct_by_radius: Vec<ChunkStatus>,
    write_radius: i32,
}

impl StepBuilder {
    fn new(status: ChunkStatus, parent: Option<&ChunkStep>) -> Self {
        let direct_by_radius = match parent {
            None => Vec::new(),
            Some(p) => vec![p.target_status],
        };
        Self {
            status,
            parent: parent.map(|p| p.target_status.index()),
            direct_by_radius,
            write_radius: -1,
        }
    }

    fn add_requirement(mut self, status: ChunkStatus, radius: i32) -> Self {
        assert!(
            status < self.status,
            "status {status:?} can not be required by {:?}",
            self.status
        );
        let previous = std::mem::take(&mut self.direct_by_radius);
        let new_length = (radius + 1) as usize;
        if new_length > previous.len() {
            self.direct_by_radius = vec![status; new_length];
        } else {
            self.direct_by_radius = vec![status; previous.len()];
        }
        for i in 0..new_length.min(previous.len()) {
            self.direct_by_radius[i] = previous[i].max(status);
        }
        // Beyond the overlap, entries keep the widened fill (`status`) when we
        // grew, or the previous values when we didn't.
        if new_length <= previous.len() {
            self.direct_by_radius[new_length..].copy_from_slice(&previous[new_length..]);
        }
        self
    }

    fn write_radius(mut self, radius: i32) -> Self {
        self.write_radius = radius;
        self
    }

    fn build(self, steps: &[ChunkStep]) -> ChunkStep {
        let accumulated = self.build_accumulated(steps);
        ChunkStep {
            target_status: self.status,
            direct_dependencies: ChunkDependencies::new(self.direct_by_radius),
            accumulated_dependencies: ChunkDependencies::new(accumulated),
            block_state_write_radius: self.write_radius,
        }
    }

    fn build_accumulated(&self, steps: &[ChunkStep]) -> Vec<ChunkStatus> {
        let Some(parent_index) = self.parent else {
            return self.direct_by_radius.clone();
        };
        let parent = &steps[parent_index];
        let radius_of_parent = self.radius_of_parent(parent.target_status);
        let parent_deps = &parent.accumulated_dependencies.dependency_by_radius;
        let len = (radius_of_parent + parent_deps.len()).max(self.direct_by_radius.len());
        let mut accumulated = Vec::with_capacity(len);
        for distance in 0..len {
            let in_parent = distance.checked_sub(radius_of_parent).filter(|d| *d < parent_deps.len());
            let value = match (in_parent, self.direct_by_radius.get(distance)) {
                (None, Some(&direct)) => direct,
                (Some(p), None) => parent_deps[p],
                (Some(p), Some(&direct)) => direct.max(parent_deps[p]),
                (None, None) => unreachable!("accumulated length covers one of the two"),
            };
            accumulated.push(value);
        }
        accumulated
    }

    fn radius_of_parent(&self, status: ChunkStatus) -> usize {
        for i in (0..self.direct_by_radius.len()).rev() {
            if self.direct_by_radius[i].is_or_after(status) {
                return i;
            }
        }
        0
    }
}

/// `ChunkPyramid` — the ordered steps, one per status.
pub struct ChunkPyramid {
    steps: Vec<ChunkStep>,
}

impl ChunkPyramid {
    pub fn step_to(&self, status: ChunkStatus) -> &ChunkStep {
        &self.steps[status.index()]
    }

    /// `ChunkPyramid.GENERATION_PYRAMID` — the vanilla requirement table.
    pub fn generation() -> &'static ChunkPyramid {
        static PYRAMID: OnceLock<ChunkPyramid> = OnceLock::new();
        PYRAMID.get_or_init(|| {
            use ChunkStatus::*;
            let mut steps: Vec<ChunkStep> = Vec::with_capacity(ChunkStatus::ALL.len());
            let step = |steps: &mut Vec<ChunkStep>, status: ChunkStatus, f: fn(StepBuilder) -> StepBuilder| {
                let builder = StepBuilder::new(status, steps.last());
                let built = f(builder).build(steps);
                steps.push(built);
            };
            step(&mut steps, Empty, |s| s);
            step(&mut steps, StructureStarts, |s| s);
            step(&mut steps, StructureReferences, |s| s.add_requirement(StructureStarts, 8));
            step(&mut steps, Biomes, |s| s.add_requirement(StructureStarts, 8));
            step(&mut steps, Noise, |s| {
                s.add_requirement(StructureStarts, 8)
                    .add_requirement(Biomes, 1)
                    .write_radius(0)
            });
            step(&mut steps, Surface, |s| {
                s.add_requirement(StructureStarts, 8)
                    .add_requirement(Biomes, 1)
                    .write_radius(0)
            });
            step(&mut steps, Carvers, |s| s.add_requirement(StructureStarts, 8).write_radius(0));
            step(&mut steps, Features, |s| {
                s.add_requirement(StructureStarts, 8)
                    .add_requirement(Carvers, 1)
                    .write_radius(1)
            });
            step(&mut steps, InitializeLight, |s| s);
            step(&mut steps, Light, |s| s.add_requirement(InitializeLight, 1));
            step(&mut steps, Spawn, |s| s.add_requirement(Biomes, 1));
            step(&mut steps, Full, |s| s);
            ChunkPyramid { steps }
        })
    }
}

// ---------------------------------------------------------------------------
// ProtoChunk
// ---------------------------------------------------------------------------

/// A chunk at an intermediate generation status (`ProtoChunk`): the pieces
/// filled in so far. `biome_sections` appears at BIOMES (per 16-block section
/// bottom-up, 4×4×4 quart biomes at container index `(y·4 + z)·4 + x`);
/// `blocks` appears at NOISE and is mutated in place by SURFACE (and, later,
/// CARVERS/FEATURES).
#[derive(Clone)]
pub struct ProtoChunk {
    pub pos: (i32, i32),
    pub status: ChunkStatus,
    pub biome_sections: Option<Vec<[u16; 64]>>,
    pub blocks: Option<FilledChunk>,
}

impl ProtoChunk {
    fn new(pos: (i32, i32)) -> Self {
        Self { pos, status: ChunkStatus::Empty, biome_sections: None, blocks: None }
    }
}

/// Chessboard (Chebyshev) distance between two chunk positions
/// (`ChunkPos.getChessboardDistance`).
pub fn chessboard_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

// ---------------------------------------------------------------------------
// WorldGenRegion
// ---------------------------------------------------------------------------

/// `WorldGenRegion` — the view a generating step works through: reads reach
/// any chunk within the step's direct-dependency radius, block writes only
/// chunks within the step's `blockStateWriteRadius`. Carvers (P7) and
/// features (P8) receive one of these.
pub struct WorldGenRegion<'a> {
    chunks: &'a mut HashMap<(i32, i32), ProtoChunk>,
    center: (i32, i32),
    step: &'a ChunkStep,
    /// `NoiseSettings.minY` / `height`, for biome-section indexing.
    min_y: i32,
    height: i32,
    /// The raw world seed and sea level (P8 decoration seeding / level queries).
    seed: i64,
    sea_level: i32,
}

impl WorldGenRegion<'_> {
    /// `hasChunk` — availability is a pure function of the generating step's
    /// dependency radius, not of what happens to be resident.
    pub fn has_chunk(&self, cx: i32, cz: i32) -> bool {
        chessboard_distance(self.center, (cx, cz)) < self.step.direct_dependencies.size()
    }

    fn chunk(&self, cx: i32, cz: i32) -> &ProtoChunk {
        self.expect_available(cx, cz);
        self.chunks.get(&(cx, cz)).expect("dependency chunk generated before this step")
    }

    fn expect_available(&self, cx: i32, cz: i32) {
        assert!(
            self.has_chunk(cx, cz),
            "requested chunk ({cx}, {cz}) unavailable while generating {:?} at {:?} \
             (dependency radius {})",
            self.step.target_status,
            self.center,
            self.step.direct_dependencies.radius(),
        );
    }

    /// The block at world `(x, y, z)`. Outside the generated y-range (or in a
    /// chunk that has not reached NOISE) everything reads as air, matching a
    /// `ProtoChunk` whose sections are still empty.
    pub fn get_block(&self, x: i32, y: i32, z: i32) -> ParityBlock {
        match &self.chunk(x >> 4, z >> 4).blocks {
            Some(blocks) if (blocks.min_y..blocks.min_y + blocks.height).contains(&y) => {
                blocks.block(x & 15, y, z & 15)
            }
            _ => ParityBlock::Air,
        }
    }

    /// Set the block at world `(x, y, z)`, enforcing the generating step's
    /// write radius: outside it the write is dropped with a warning, exactly
    /// vanilla's `ensureCanWrite` behavior. Returns whether the write landed.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: ParityBlock) -> bool {
        let pos = (x >> 4, z >> 4);
        let dx = (self.center.0 - pos.0).abs();
        let dz = (self.center.1 - pos.1).abs();
        if dx > self.step.block_state_write_radius || dz > self.step.block_state_write_radius {
            warn!(
                x, y, z,
                center = ?self.center,
                status = ?self.step.target_status,
                "detected setBlock in a far chunk during worldgen; dropping the write"
            );
            return false;
        }
        let chunk = self.chunks.get_mut(&pos).expect("writable chunk resident");
        let blocks = chunk.blocks.as_mut().expect("writable chunk past NOISE");
        if !(blocks.min_y..blocks.min_y + blocks.height).contains(&y) {
            return false;
        }
        blocks.set_block(x & 15, y, z & 15, state);
        true
    }

    /// `getNoiseBiome` at quart coordinates — read from the owning chunk's
    /// *stored* biome sections (the chunk is ≥ BIOMES by the pyramid).
    pub fn get_noise_biome(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> u16 {
        let chunk = self.chunk(quart_x >> 2, quart_z >> 2);
        let sections = chunk.biome_sections.as_ref().expect("biome-dependency chunk ≥ BIOMES");
        let min_section_y = self.min_y >> 4;
        let qy = quart_y.clamp(self.min_y >> 2, (self.min_y + self.height - 1) >> 2);
        let section = &sections[((qy >> 2) - min_section_y) as usize];
        section[((((qy & 3) * 4) + (quart_z & 3)) * 4 + (quart_x & 3)) as usize]
    }

    /// The `FilledChunk` at a chunk position, without the `has_chunk` assertion
    /// (a chunk outside the read radius, or below NOISE, reads as absent).
    fn filled(&self, cx: i32, cz: i32) -> Option<&FilledChunk> {
        self.chunks.get(&(cx, cz)).and_then(|c| c.blocks.as_ref())
    }
}

// P8 — the feature/decoration view onto the region. Reads route to the owning
// chunk's stored blocks / heightmaps; writes honor the FEATURES write radius.
impl DecorationLevel for WorldGenRegion<'_> {
    fn get_block(&self, x: i32, y: i32, z: i32) -> ParityBlock {
        WorldGenRegion::get_block(self, x, y, z)
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: ParityBlock) -> bool {
        WorldGenRegion::set_block(self, x, y, z, state)
    }

    fn get_height(&self, heightmap: Heightmap, x: i32, z: i32) -> i32 {
        let column = ((z & 15) * 16 + (x & 15)) as usize;
        let Some(fc) = self.filled(x >> 4, z >> 4) else {
            return self.min_y;
        };
        match heightmap {
            // The worldgen heightmaps are computed during `doFill`.
            Heightmap::WorldSurfaceWg => fc.world_surface_wg[column],
            Heightmap::OceanFloorWg => fc.ocean_floor_wg[column],
            // The FINAL heightmaps are, in vanilla, primed at FEATURES start and
            // then maintained by `setBlockState`. For an add-mostly feature pass
            // that is equivalent to a fresh top-down scan of the current blocks,
            // which is what we do (no separate heightmap state to maintain).
            _ => {
                let mut y = fc.min_y + fc.height - 1;
                while y >= fc.min_y {
                    if heightmap.matches(fc.block(x & 15, y, z & 15)) {
                        return y + 1;
                    }
                    y -= 1;
                }
                fc.min_y
            }
        }
    }

    fn get_biome_fill(&self, x: i32, y: i32, z: i32) -> u16 {
        // `WorldGenRegion.getBiome` → BiomeManager fuzzy zoom (`obfuscateSeed`,
        // fiddled-distance corner pick) over the region's stored quart biomes.
        let (qx, qy, qz) = zoomed_quart(obfuscate_seed(self.seed), x, y, z);
        self.get_noise_biome(qx, qy, qz)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn gen_depth(&self) -> i32 {
        self.height
    }

    fn sea_level(&self) -> i32 {
        self.sea_level
    }
}

/// The process-wide overworld feature registry (parsed once from the vendored
/// datapack). Keyed on nothing: the overworld biome order is fixed, so the
/// `FeatureSorter` output is deterministic.
fn overworld_feature_registry(source: &MultiNoiseBiomeSource) -> &'static FeatureRegistry {
    static REG: OnceLock<FeatureRegistry> = OnceLock::new();
    REG.get_or_init(|| {
        let names = (0..source.biome_count()).map(|i| source.biome_name(i as u16).to_owned()).collect();
        FeatureRegistry::load(names)
    })
}

// ---------------------------------------------------------------------------
// The pipeline scheduler
// ---------------------------------------------------------------------------

/// The staged generator: proto-chunks by position plus the seeded P2–P5
/// engines. `advance` brings a chunk to a target status, generating every
/// dependency (recursively, depth-first) first — the vanilla "pyramid".
///
/// One instance is single-threaded by design: parity output must not depend on
/// scheduling *within* an instance, so a deterministic depth-first order is the
/// simplest correct scheduler. (The only order-visible state is the RTree's
/// last-result tie-breaking seed, which vanilla itself leaves thread-dependent
/// via a `ThreadLocal` — and which never changes a non-tie result.)
///
/// The live path runs one instance *per prefetch worker* (see [`with_pipeline`])
/// rather than sharing one behind a lock: instances are independent, so distinct
/// workers generate in parallel, and byte-identical output for a `(seed, pos)`
/// is preserved because each instance is a pure function of the seed save for
/// the same measure-zero tie-break vanilla already leaves per-thread.
pub struct ChunkPipeline {
    pub generator: SurfacedGenerator,
    /// The raw world seed — `applyCarvers` (P7) re-seeds off `seed + carverIndex`,
    /// and `setDecorationSeed` (P8) needs it.
    seed: i64,
    chunks: HashMap<(i32, i32), ProtoChunk>,
}

impl ChunkPipeline {
    pub fn new_overworld(seed: i64) -> Self {
        Self {
            generator: SurfacedGenerator::new_overworld(seed),
            seed,
            chunks: HashMap::new(),
        }
    }

    /// The proto-chunk at `pos`, if it has been touched.
    pub fn chunk(&self, cx: i32, cz: i32) -> Option<&ProtoChunk> {
        self.chunks.get(&(cx, cz))
    }

    fn status(&mut self, pos: (i32, i32)) -> ChunkStatus {
        self.chunks.entry(pos).or_insert_with(|| ProtoChunk::new(pos)).status
    }

    /// Advance chunk `(cx, cz)` to `target`, generating dependencies first.
    /// Idempotent: a chunk already at or past `target` is untouched.
    pub fn advance(&mut self, cx: i32, cz: i32, target: ChunkStatus) -> &ProtoChunk {
        let pos = (cx, cz);
        let mut current = self.status(pos);
        while current < target {
            let next = ChunkStatus::ALL[current.index() + 1];
            let step = ChunkPyramid::generation().step_to(next);
            // Neighbors first: at each chessboard distance the step's direct
            // dependencies name the status that ring must have reached.
            // (Distance 0 is `pos` itself at the parent status — the previous
            // loop iteration.)
            for distance in 1..step.direct_dependencies.size() {
                let required = step.direct_dependencies.get(distance);
                for dz in -distance..=distance {
                    for dx in -distance..=distance {
                        if dx.abs().max(dz.abs()) == distance {
                            self.advance(cx + dx, cz + dz, required);
                        }
                    }
                }
            }
            self.run_step(pos, step);
            let chunk = self.chunks.get_mut(&pos).expect("proto chunk resident");
            chunk.status = next;
            current = next;
        }
        self.chunks.get(&pos).expect("proto chunk resident")
    }

    /// One `ChunkStatusTasks` stage. Statuses without a P-layer yet are
    /// vanilla-shaped no-ops (see the module docs).
    fn run_step(&mut self, pos: (i32, i32), step: &ChunkStep) {
        let noise = self.generator.inner.random_state.settings.noise;
        match step.target_status {
            // `generateBiomes` → `fillBiomesFromNoise`.
            ChunkStatus::Biomes => {
                let sections = self.generator.source.fill_chunk_biomes(
                    &self.generator.sampler,
                    pos.0,
                    pos.1,
                    noise.min_y,
                    noise.height,
                );
                self.chunks.get_mut(&pos).expect("proto chunk resident").biome_sections =
                    Some(sections);
            }
            // `generateNoise` → `doFill` (aquifers + ore veins included).
            ChunkStatus::Noise => {
                let filled = self.generator.inner.fill_chunk(pos.0, pos.1);
                self.chunks.get_mut(&pos).expect("proto chunk resident").blocks = Some(filled);
            }
            // `generateSurface` → `buildSurface` over the biomes *stored* in
            // the 3×3 neighborhood (guaranteed ≥ BIOMES by this step's
            // dependencies), the staged equivalent of vanilla's BiomeManager
            // reading neighbor chunks through the region.
            ChunkStatus::Surface => {
                let baked = {
                    let sections = |dx: i32, dz: i32| {
                        let chunk = &self.chunks[&(pos.0 + dx, pos.1 + dz)];
                        ((pos.0 + dx, pos.1 + dz), chunk.biome_sections.as_deref().expect("neighbors ≥ BIOMES"))
                    };
                    let mut all = Vec::with_capacity(9);
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            all.push(sections(dx, dz));
                        }
                    }
                    BakedBiomes::from_sections(all, noise.min_y, noise.height)
                };
                let mut noise_chunk = NoiseChunk::for_chunk(
                    &self.generator.inner.random_state,
                    pos.0 * 16,
                    pos.1 * 16,
                );
                let chunk = self.chunks.get_mut(&pos).expect("proto chunk resident");
                let blocks = chunk.blocks.as_mut().expect("SURFACE runs after NOISE");
                self.generator.surface.build_surface(blocks, &mut noise_chunk, &baked, pos.0, pos.1);
            }
            // `generateCarvers` → `applyCarvers`. Carve the center chunk in
            // place from every start-chunk carver in its 17×17 neighborhood
            // (write radius 0). The blocks are taken out so the generator (read
            // for the biome source + aquifer) and the mutated chunk don't alias.
            ChunkStatus::Carvers => {
                // The 3×3 stored biomes feed `carveBlock`'s top-material fix-up
                // (dirt under a carved grass/mycelium column → biome top). The
                // neighborhood is ≥ BIOMES because the center's SURFACE step
                // (this step's parent) already required Biomes at radius 1.
                let baked = {
                    let mut all = Vec::with_capacity(9);
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            let chunk = &self.chunks[&(pos.0 + dx, pos.1 + dz)];
                            all.push((
                                (pos.0 + dx, pos.1 + dz),
                                chunk.biome_sections.as_deref().expect("neighbors ≥ BIOMES"),
                            ));
                        }
                    }
                    BakedBiomes::from_sections(all, noise.min_y, noise.height)
                };
                let mut blocks = self
                    .chunks
                    .get_mut(&pos)
                    .expect("proto chunk resident")
                    .blocks
                    .take()
                    .expect("CARVERS runs after NOISE/SURFACE");
                super::carvers::apply_carvers(&self.generator, self.seed, pos, &mut blocks, &baked);
                self.chunks.get_mut(&pos).expect("proto chunk resident").blocks = Some(blocks);
            }
            // `generateFeatures` → `applyBiomeDecoration` (P8). Features of every
            // biome present in the chunk's 3×3 section neighborhood are unioned
            // (FeatureSorter global order), seeded per feature, and placed
            // through a write-radius-1 region.
            ChunkStatus::Features => {
                let settings = &self.generator.inner.random_state.settings;
                let (min_y, height, sea_level, seed) =
                    (noise.min_y, noise.height, settings.sea_level, self.seed);
                let registry = overworld_feature_registry(&self.generator.source);

                // `possibleBiomes`: every fill value in the 3×3 neighborhood's
                // stored biome sections (vanilla unions `getBiomes` over the
                // ChunkPos.rangeClosed(±1) chunks).
                let mut possible: HashSet<u16> = HashSet::new();
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if let Some(chunk) = self.chunks.get(&(pos.0 + dx, pos.1 + dz)) {
                            if let Some(sections) = &chunk.biome_sections {
                                for section in sections {
                                    possible.extend(section.iter().copied());
                                }
                            }
                        }
                    }
                }

                let mut region = WorldGenRegion {
                    chunks: &mut self.chunks,
                    center: pos,
                    step,
                    min_y,
                    height,
                    seed,
                    sea_level,
                };
                features::apply_biome_decoration(
                    registry,
                    &mut region,
                    &possible,
                    seed,
                    pos.0 * 16,
                    pos.1 * 16,
                );
            }
            // P9 (structure starts/references): no-op contributions are exactly
            // vanilla wherever those systems would not have touched the chunk.
            // Light and spawn live outside the worldgen parity layer (encode-time
            // light, sim spawning).
            _ => {}
        }
    }

    /// The generating region for a step at `pos` — what a carver/feature
    /// implementation receives. Exposed for the P7/P8 layers and tests.
    pub fn region_for<'a>(&'a mut self, pos: (i32, i32), step: &'a ChunkStep) -> WorldGenRegion<'a> {
        let settings = &self.generator.inner.random_state.settings;
        let noise = settings.noise;
        let sea_level = settings.sea_level;
        let seed = self.seed;
        WorldGenRegion {
            chunks: &mut self.chunks,
            center: pos,
            step,
            min_y: noise.min_y,
            height: noise.height,
            seed,
            sea_level,
        }
    }

    /// Run the FEATURES decoration for chunk `pos` over `chunks` — a scratch map
    /// (not `self.chunks`) holding the CARVERS-state neighborhood. Identical to
    /// the staged FEATURES `run_step` branch, but parameterized over the map so a
    /// served chunk can be decorated in isolation without ever featuring the
    /// shared cache (which must stay a pure, radius-0 CARVERS function — see
    /// [`Self::feature_extract`]). Borrows `self` immutably (settings + registry)
    /// alongside the separate `&mut chunks`.
    fn decorate_into(&self, chunks: &mut HashMap<(i32, i32), ProtoChunk>, pos: (i32, i32)) {
        let settings = &self.generator.inner.random_state.settings;
        let noise = settings.noise;
        let (min_y, height, sea_level, seed) = (noise.min_y, noise.height, settings.sea_level, self.seed);
        let registry = overworld_feature_registry(&self.generator.source);
        let step = ChunkPyramid::generation().step_to(ChunkStatus::Features);

        // `possibleBiomes`: every fill value in the 3×3 neighborhood's stored
        // biome sections (exactly the staged FEATURES step's union).
        let mut possible: HashSet<u16> = HashSet::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                if let Some(chunk) = chunks.get(&(pos.0 + dx, pos.1 + dz)) {
                    if let Some(sections) = &chunk.biome_sections {
                        for section in sections {
                            possible.extend(section.iter().copied());
                        }
                    }
                }
            }
        }

        let mut region =
            WorldGenRegion { chunks, center: pos, step, min_y, height, seed, sea_level };
        features::apply_biome_decoration(registry, &mut region, &possible, seed, pos.0 * 16, pos.1 * 16);
    }

    /// Produce the fully-featured center chunk `(cx, cz)` for the live world.
    ///
    /// FEATURES has `blockStateWriteRadius` 1, so `(cx, cz)`'s final blocks are
    /// the union of its own decoration and every one of its 8 neighbors'
    /// decoration writing into it. The naive fix — feature the shared cache in
    /// place and serve at FULL — is unusable here: (a) the live column loader
    /// (`ChunkData::from_grid`) regenerates the baseline on *every* load and
    /// stores edits as a diff against it, so `generate_full` MUST be a
    /// deterministic pure function of the seed, but in-place cross-chunk feature
    /// writes make a chunk's blocks depend on the order neighbors decorate into
    /// it; and (b) `advance(_, Full)` pulls a 17×17 neighborhood
    /// (`structure_starts` radius 8), so per-serve regeneration is far too slow.
    ///
    /// Instead: bring the 5×5 to CARVERS in the shared cache (safe to amortize —
    /// CARVERS is write-radius 0, a pure function of the seed), then **clone** it
    /// and decorate the 3×3 on the copy in a fixed order — the center first, then
    /// the distance-1 ring in `dz`-outer/`dx`-inner order. Every serve clones the
    /// identical CARVERS base and applies the identical decoration order, so the
    /// featured center is a pure function of the seed (the invariant
    /// `from_grid` needs); the shared cache is never featured, so it stays pure
    /// and keeps amortizing across serves. A cross-border feature is captured in
    /// full for the chunk it overhangs into, because that neighbor is one of the
    /// 3×3 decorated here.
    ///
    /// KNOWN GAP (not yet vanilla-exact, deferred to the serve-once lifecycle +
    /// `.mca` diff): the decoration order here is per-serve-local, not the global
    /// chunk-generation order. When serving C the center decorates first (reading
    /// clean CARVERS); when serving a neighbor N the same center decorates
    /// mid-sequence (reading N's partial writes). If a cross-border feature's
    /// placement validity depends on those differing reads, the two serves can
    /// disagree on that feature — a possible seam at chunk boundaries. Ores,
    /// springs, disks, and any feature that stays within one chunk are unaffected
    /// (no cross-border read/write); most trees place identically (trunk validity
    /// keys off the CARVERS ground, which is shared). Border-exactness needs each
    /// chunk decorated exactly once in global order — the full FULL-lifecycle
    /// integration tracked in docs/WORLDGEN_PARITY.md.
    fn feature_extract(&mut self, cx: i32, cz: i32) -> ProtoChunk {
        // 1. Base terrain: the 5×5 at CARVERS. Features read/write radius 1 from
        //    each of the 3×3, reaching distance 2. Amortized by the shared cache.
        for dz in -2..=2 {
            for dx in -2..=2 {
                self.advance(cx + dx, cz + dz, ChunkStatus::Carvers);
            }
        }
        // 2. Clone the 5×5 CARVERS neighborhood into a scratch map.
        let mut scratch: HashMap<(i32, i32), ProtoChunk> = HashMap::with_capacity(25);
        for dz in -2..=2 {
            for dx in -2..=2 {
                let p = (cx + dx, cz + dz);
                scratch.insert(p, self.chunks[&p].clone());
            }
        }
        // 3. Decorate the 3×3 into the copy in canonical order: center, then the
        //    distance-1 ring. Each chunk's radius-1 writes/reads stay inside the
        //    5×5, and later chunks see earlier writes exactly as the staged pass.
        self.decorate_into(&mut scratch, (cx, cz));
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx != 0 || dz != 0 {
                    self.decorate_into(&mut scratch, (cx + dx, cz + dz));
                }
            }
        }
        scratch.remove(&(cx, cz)).expect("center chunk resident in scratch")
    }
}

// ---------------------------------------------------------------------------
// The live path (behind VELA_PARITY_WORLDGEN)
// ---------------------------------------------------------------------------

/// One chunk extracted from the pipeline for the live world: the surfaced
/// blocks plus the per-column *surface* biome (the parameter-list fill value
/// at each column's top-block quart). The 2-D projection is a wire-side
/// simplification — Vela's chunk encoder still sends one biome per column, so
/// cave biomes aren't visible client-side yet; the stored 3-D quarts remain
/// in the pipeline for worldgen itself.
pub struct ParityChunk {
    pub blocks: FilledChunk,
    pub surface_biomes: [u16; 256],
}

// One pipeline per thread, so the prefetch worker pool generates in parallel
// with zero lock contention. `ChunkPipeline` is `!Send` (its P2–P5 engines
// hold `Rc`s and its RTree keeps an interior-mutable last-result cell), which
// is exactly why *sharing* one instance across threads was a mutex before —
// here each thread owns its instance outright and nothing crosses the thread
// boundary, so the `!Send` bound is a perfect fit rather than an obstacle.
//
// Determinism is unaffected: output is a pure function of `(seed, pos)` save
// for the RTree's tie-break on fitness ties (a measure-zero event on real
// noise), which vanilla itself makes thread-dependent via a `ThreadLocal`
// last-result. One pipeline per worker reproduces that per-thread evolution
// exactly — strictly closer to vanilla than a single shared instance.
//
// The instance is seeded lazily from `super::seed`, which every worker reads
// after the boot-time `super::set_seed`, so all workers agree on the seed.
thread_local! {
    static PIPELINE: std::cell::RefCell<ChunkPipeline> =
        std::cell::RefCell::new(ChunkPipeline::new_overworld(super::seed() as i64));
}

/// Run `f` against the calling thread's pipeline (creating it on first touch).
fn with_pipeline<R>(f: impl FnOnce(&mut ChunkPipeline) -> R) -> R {
    PIPELINE.with(|cell| f(&mut cell.borrow_mut()))
}

/// Above this many resident proto-chunks, trim the trail: keep only the
/// neighborhood of the chunk just consumed. Dropped protos regenerate
/// deterministically if revisited; this only bounds memory (a surfaced proto
/// is ~100 KiB), it never changes output.
///
/// The cache is now per worker thread (see [`with_pipeline`]), so the bound is
/// paid once per prefetch worker: at most `prefetch_workers() × PROTO_CACHE_LIMIT`
/// protos live at once. The worker pool is no longer capped at 8 (it now runs
/// ~`num_cpus - 2`, e.g. 22 on a 24-core host), so the per-worker limit is
/// lowered to keep aggregate memory bounded: 320 protos ≈ 31 MiB per thread
/// (~100 KiB each), ~690 MiB across a 22-worker pool — up from ~400 MiB for
/// far more throughput. `PROTO_KEEP_RADIUS` keeps a 17×17 (289-proto) working
/// set on each trim, which must stay under the limit so trims have hysteresis
/// rather than thrashing; 320 sits just above 289 to keep that margin while
/// holding aggregate memory down. Spatial sharding in `prefetch` keeps each
/// worker's moves local, so this smaller cache still amortizes the shared
/// biome/noise rings across a worker's neighborhood.
const PROTO_CACHE_LIMIT: usize = 320;
const PROTO_KEEP_RADIUS: i32 = 8;

/// The shared cache is held at CARVERS: caves/ravines (P7) run and, crucially,
/// CARVERS has `blockStateWriteRadius` 0 — it only mutates its own chunk — so a
/// cached proto is a deterministic pure function of the seed, unaffected by
/// generation/cache/eviction order. That is what makes the trail-trim eviction
/// above sound. FEATURES (write radius 1) is applied per serve on an isolated
/// clone of the neighborhood ([`ChunkPipeline::feature_extract`]) and is never
/// written back into the shared cache, so the cache's purity is preserved.
///
/// Heightmap handling matches vanilla across the surface→carvers boundary
/// (verified against `ChunkStatusTasks`/`Heightmap.primeHeightmaps`): the WG
/// heightmaps (`OCEAN_FLOOR_WG`/`WORLD_SURFACE_WG`) are built at NOISE and are
/// **not** re-primed after carving — vanilla's carvers touch only the FINAL
/// heightmaps, and the WG pair keeps its pre-carve value (which is what
/// `surface_height`/spawn read). Light/spawn/FULL stay no-ops (Vela lights at
/// encode time, spawns in the sim).
///
/// Generate chunk `(cx, cz)` through the pipeline — CARVERS base terrain in the
/// shared cache, then FEATURES decoration on an isolated copy (see
/// [`ChunkPipeline::feature_extract`]) — and extract it. The shared cache keeps
/// its CARVERS-state protos resident so the dependency pyramid amortizes as the
/// player moves; the featured center is a pure function of the seed.
pub fn generate_full(cx: i32, cz: i32) -> ParityChunk {
    with_pipeline(|pipeline| {
        let proto = pipeline.feature_extract(cx, cz);
        let blocks = proto.blocks.expect("surfaced chunk has blocks");
        let sections = proto.biome_sections.expect("surfaced chunk has biomes");

        let min_section_y = blocks.min_y >> 4;
        let quart_min_y = blocks.min_y >> 2;
        let quart_max_y = (blocks.min_y + blocks.height - 1) >> 2;
        let mut surface_biomes = [0u16; 256];
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let col = (lz * 16 + lx) as usize;
                let top = (blocks.world_surface_wg[col] - 1).max(blocks.min_y);
                let (qx, qz) = ((cx * 16 + lx) >> 2, (cz * 16 + lz) >> 2);
                let qy = (top >> 2).clamp(quart_min_y, quart_max_y);
                let section = &sections[((qy >> 2) - min_section_y) as usize];
                surface_biomes[col] =
                    section[((((qy & 3) * 4) + (qz & 3)) * 4 + (qx & 3)) as usize];
            }
        }

        if pipeline.chunks.len() > PROTO_CACHE_LIMIT {
            pipeline
                .chunks
                .retain(|pos, _| chessboard_distance(*pos, (cx, cz)) <= PROTO_KEEP_RADIUS);
        }
        ParityChunk { blocks, surface_biomes }
    })
}

/// The terrain surface height (topmost solid block y) at a world column,
/// parity edition: `OCEAN_FLOOR_WG` (fluids excluded, like the legacy
/// height field) read from the pipeline after surfacing the owning chunk.
/// Cached by the proto cache, so repeated probes in one area are cheap.
pub fn surface_height(wx: i32, wz: i32) -> i32 {
    with_pipeline(|pipeline| {
        let (cx, cz) = (wx >> 4, wz >> 4);
        pipeline.advance(cx, cz, ChunkStatus::Surface);
        let blocks =
            pipeline.chunks[&(cx, cz)].blocks.as_ref().expect("surfaced chunk has blocks");
        blocks.ocean_floor_wg[((wz & 15) * 16 + (wx & 15)) as usize] - 1
    })
}

/// The real block-state id for a parity block, resolved once per variant from
/// the block registry (unresolvable names fall back to stone, keeping
/// generation total — same policy as the legacy palette in `blocks.rs`).
pub fn block_state_of(block: ParityBlock) -> crate::ids::BlockState {
    static TABLE: OnceLock<[crate::ids::BlockState; ParityBlock::ALL.len()]> = OnceLock::new();
    TABLE.get_or_init(|| {
        ParityBlock::ALL.map(|b| {
            let fallback = if b == ParityBlock::Air { 0 } else { 1 };
            crate::ids::BlockState(
                crate::registry::block_state::default_state_of(b.block_name()).unwrap_or(fallback),
            )
        })
    })[block as usize]
}

/// Per parameter-list fill value: the biome's registry name (leaked once —
/// the set is the 55 parameter-list biomes) and its synced network id.
fn biome_table() -> &'static Vec<(&'static str, u32)> {
    static TABLE: OnceLock<Vec<(&'static str, u32)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        with_pipeline(|pipeline| {
            (0..pipeline.generator.source.biome_count())
                .map(|i| {
                    let name: &'static str = Box::leak(
                        pipeline.generator.source.biome_name(i as u16).to_owned().into(),
                    );
                    (name, super::biome::network_id(name))
                })
                .collect()
        })
    })
}

/// The `minecraft:` biome id for a stored fill value (disk biome palettes).
pub fn biome_name_of(fill: u16) -> &'static str {
    biome_table()[fill as usize].0
}

/// The synced-registry network index for a stored fill value (wire palettes).
pub fn biome_network_id_of(fill: u16) -> u32 {
    biome_table()[fill as usize].1
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChunkStatus::*;

    /// End-to-end: advancing a chunk to FEATURES runs P8 decoration through the
    /// real `WorldGenRegion`. Feature ores (e.g. `CoalOre`, distinct from the P3
    /// vein blocks) appear, and the whole block grid is a deterministic function
    /// of the seed.
    #[test]
    fn features_stage_places_ores_deterministically() {
        let feature_ore_counts = |seed: i64| -> (usize, Vec<ParityBlock>) {
            let mut p = ChunkPipeline::new_overworld(seed);
            p.advance(0, 0, Features);
            let chunk = p.chunk(0, 0).expect("center chunk");
            assert_eq!(chunk.status, Features);
            let fc = chunk.blocks.as_ref().expect("filled");
            let coal = fc.blocks.iter().filter(|b| **b == ParityBlock::CoalOre).count();
            (coal, fc.blocks.clone())
        };
        let (coal_a, grid_a) = feature_ore_counts(1234);
        let (coal_b, grid_b) = feature_ore_counts(1234);
        assert!(coal_a > 0, "the underground_ores step placed coal ore");
        assert_eq!(coal_a, coal_b, "coal count is deterministic");
        assert_eq!(grid_a, grid_b, "the whole feature-decorated grid is deterministic");

        let (_, grid_c) = feature_ore_counts(5678);
        assert_ne!(grid_a, grid_c, "a different seed decorates differently");
    }

    /// A FEATURES chunk differs from its SURFACE (pre-decoration) state — the
    /// decoration pass actually wrote blocks into the center chunk.
    #[test]
    fn features_modify_the_chunk() {
        let mut surf = ChunkPipeline::new_overworld(99);
        surf.advance(0, 0, Surface);
        let before = surf.chunk(0, 0).unwrap().blocks.as_ref().unwrap().blocks.clone();

        let mut feat = ChunkPipeline::new_overworld(99);
        feat.advance(0, 0, Features);
        let after = feat.chunk(0, 0).unwrap().blocks.as_ref().unwrap().blocks.clone();

        assert_ne!(before, after, "decoration changed the center chunk");
    }

    /// Trees reach the live world: scanning a forested region of the live
    /// FEATURES path turns up log and leaf blocks (and leaves outnumber logs, as
    /// a real canopy does). Guards against a regression that silently drops the
    /// tree feature back out of the live pipeline.
    #[test]
    fn live_features_grow_trees() {
        use ParityBlock::*;
        // Seed 0's origin region is densely forested (verified: 135/144 chunks).
        let mut p = ChunkPipeline::new_overworld(0);
        let (mut logs, mut leaves) = (0u64, 0u64);
        for cz in -2..2 {
            for cx in -2..2 {
                let fc = p.feature_extract(cx, cz).blocks.expect("featured blocks");
                logs += fc.blocks.iter().filter(|b| matches!(b, OakLog | BirchLog | SpruceLog | DarkOakLog)).count() as u64;
                leaves += fc.blocks.iter().filter(|b| b.is_leaves()).count() as u64;
            }
        }
        assert!(logs > 0, "the live FEATURES path placed tree logs");
        assert!(leaves > logs, "leaves form a canopy over the logs");
    }

    /// Fancy oak reaches the live pipeline: `fancy_oak` is ~10% of oak spawns in
    /// forest/plains, so a forested region of the live FEATURES path grows oaks
    /// (logs + leaves), and the whole pass is a pure function of the seed
    /// (byte-identical across two independent runs). Fancy oaks write plain
    /// `OakLog`/`OakLeaves`, indistinguishable from straight oaks at the block
    /// grid, so this asserts oak presence + determinism; the block-exact fancy
    /// trunk/canopy shape is covered by the `features.rs` unit test.
    #[test]
    fn live_features_grow_fancy_oaks() {
        use ParityBlock::*;
        let scan = || {
            let mut p = ChunkPipeline::new_overworld(0);
            let (mut logs, mut leaves) = (0u64, 0u64);
            for cz in -2..2 {
                for cx in -2..2 {
                    let fc = p.feature_extract(cx, cz).blocks.expect("featured blocks");
                    logs += fc.blocks.iter().filter(|b| matches!(b, OakLog)).count() as u64;
                    leaves += fc.blocks.iter().filter(|b| matches!(b, OakLeaves)).count() as u64;
                }
            }
            (logs, leaves)
        };
        let (logs, leaves) = scan();
        assert!(logs > 0, "the live FEATURES path placed oak logs");
        assert!(leaves > 0, "oak leaves appear");
        assert_eq!(scan(), (logs, leaves), "the featured oak output is deterministic across runs");
    }

    /// The live FEATURES path (`feature_extract`) is a pure function of the seed:
    /// the featured center is byte-identical whether or not other chunks were
    /// served first. This is the invariant that keeps `generate_full` safe for
    /// the column loader, which diffs saved edits against a *regenerated*
    /// baseline (`ChunkData::from_grid`) — a serve-order-dependent baseline would
    /// silently corrupt saved worlds. It also confirms the pass actually
    /// decorates (the featured center differs from the CARVERS-only blocks).
    #[test]
    fn feature_extract_is_serve_order_independent() {
        let seed = 4242;
        // Cold: a fresh pipeline serves (0, 0) directly.
        let mut cold = ChunkPipeline::new_overworld(seed);
        let a = cold.feature_extract(0, 0).blocks.expect("featured blocks");

        // Warm: serve a spread of other chunks first, then (0, 0). The shared
        // CARVERS cache is populated and trail state differs, yet the featured
        // center must be identical.
        let mut warm = ChunkPipeline::new_overworld(seed);
        for (cx, cz) in [(5, 5), (-3, 2), (1, -4), (0, 1), (-1, -1), (2, 2)] {
            let _ = warm.feature_extract(cx, cz);
        }
        let b = warm.feature_extract(0, 0).blocks.expect("featured blocks");
        assert_eq!(a.blocks, b.blocks, "featured center is independent of serve order");

        // It genuinely decorated: the featured center differs from CARVERS-only.
        let mut carv = ChunkPipeline::new_overworld(seed);
        carv.advance(0, 0, Carvers);
        let base = carv.chunk(0, 0).unwrap().blocks.as_ref().unwrap().blocks.clone();
        assert_ne!(a.blocks, base, "decoration wrote blocks into the center");
    }

    /// The vanilla GENERATION_PYRAMID dependency tables, derived by hand from
    /// `ChunkStep.Builder` (`addRequirement` + `buildAccumulatedDependencies`)
    /// applied to the `ChunkPyramid.GENERATION_PYRAMID` builder calls.
    #[test]
    fn generation_pyramid_matches_vanilla() {
        let p = ChunkPyramid::generation();
        let ss8 = |head: &[ChunkStatus]| {
            let mut v = head.to_vec();
            v.extend(std::iter::repeat(StructureStarts).take(9 - head.len().min(9)));
            v
        };

        // Direct dependencies.
        assert_eq!(p.step_to(Empty).direct_dependencies.as_slice(), &[] as &[ChunkStatus]);
        assert_eq!(p.step_to(StructureStarts).direct_dependencies.as_slice(), &[Empty]);
        assert_eq!(p.step_to(StructureReferences).direct_dependencies.as_slice(), vec![StructureStarts; 9]);
        assert_eq!(p.step_to(Biomes).direct_dependencies.as_slice(), ss8(&[StructureReferences]));
        assert_eq!(p.step_to(Noise).direct_dependencies.as_slice(), ss8(&[Biomes, Biomes]));
        assert_eq!(p.step_to(Surface).direct_dependencies.as_slice(), ss8(&[Noise, Biomes]));
        assert_eq!(p.step_to(Carvers).direct_dependencies.as_slice(), ss8(&[Surface]));
        assert_eq!(p.step_to(Features).direct_dependencies.as_slice(), ss8(&[Carvers, Carvers]));
        assert_eq!(p.step_to(InitializeLight).direct_dependencies.as_slice(), &[Features]);
        assert_eq!(p.step_to(Light).direct_dependencies.as_slice(), &[InitializeLight, InitializeLight]);
        assert_eq!(p.step_to(Spawn).direct_dependencies.as_slice(), &[Light, Biomes]);
        assert_eq!(p.step_to(Full).direct_dependencies.as_slice(), &[Spawn]);

        // Accumulated dependencies (the "pyramid").
        let tail_ss = |head: &[ChunkStatus], len: usize| {
            let mut v = head.to_vec();
            v.extend(std::iter::repeat(StructureStarts).take(len - head.len()));
            v
        };
        assert_eq!(p.step_to(StructureStarts).accumulated_dependencies.as_slice(), &[Empty]);
        assert_eq!(
            p.step_to(Noise).accumulated_dependencies.as_slice(),
            tail_ss(&[Biomes, Biomes], 10)
        );
        assert_eq!(
            p.step_to(Surface).accumulated_dependencies.as_slice(),
            tail_ss(&[Noise, Biomes], 10)
        );
        assert_eq!(
            p.step_to(Carvers).accumulated_dependencies.as_slice(),
            tail_ss(&[Surface, Biomes], 10)
        );
        assert_eq!(
            p.step_to(Features).accumulated_dependencies.as_slice(),
            tail_ss(&[Carvers, Carvers, Biomes], 11)
        );
        assert_eq!(
            p.step_to(Light).accumulated_dependencies.as_slice(),
            tail_ss(&[InitializeLight, InitializeLight, Carvers, Biomes], 12)
        );
        assert_eq!(
            p.step_to(Full).accumulated_dependencies.as_slice(),
            tail_ss(&[Spawn, InitializeLight, Carvers, Biomes], 12)
        );

        // Write radii.
        for (status, radius) in [
            (Empty, -1), (StructureStarts, -1), (StructureReferences, -1), (Biomes, -1),
            (Noise, 0), (Surface, 0), (Carvers, 0), (Features, 1),
            (InitializeLight, -1), (Light, -1), (Spawn, -1), (Full, -1),
        ] {
            assert_eq!(p.step_to(status).block_state_write_radius, radius, "{status:?}");
        }

        // Inverse lookups.
        assert_eq!(p.step_to(Full).accumulated_radius_of(StructureStarts), 11);
        assert_eq!(p.step_to(Full).accumulated_radius_of(Biomes), 3);
        assert_eq!(p.step_to(Full).accumulated_radius_of(Carvers), 2);
        assert_eq!(p.step_to(Full).accumulated_radius_of(Full), 0);
        assert_eq!(p.step_to(Features).direct_dependencies.radius_of(Carvers), 1);
        assert_eq!(p.step_to(Features).direct_dependencies.radius_of(StructureStarts), 8);
    }

    /// Every parity block resolves to a distinct real block-state id (a
    /// missing registry name would collapse onto the stone/air fallback and
    /// collide), and every parameter-list biome resolves to a distinct synced
    /// network id (a missing biome would collapse onto index 0).
    #[test]
    fn live_mapping_tables_resolve() {
        let mut block_ids: Vec<_> = ParityBlock::ALL.iter().map(|&b| block_state_of(b)).collect();
        block_ids.sort_by_key(|s| s.0);
        block_ids.dedup();
        assert_eq!(block_ids.len(), ParityBlock::ALL.len(), "parity block mapping collided");
        assert_eq!(block_state_of(ParityBlock::Air).0, 0);

        let count = with_pipeline(|p| p.generator.source.biome_count());
        let mut biome_ids: Vec<_> = (0..count as u16).map(biome_network_id_of).collect();
        biome_ids.sort_unstable();
        biome_ids.dedup();
        assert_eq!(biome_ids.len(), count, "parity biome mapping collided");
        for fill in 0..count as u16 {
            assert!(biome_name_of(fill).starts_with("minecraft:"));
        }
    }

    /// The live extraction path: a FULL chunk leaves the cache with blocks
    /// and a plausible per-column surface biome, and the parity
    /// `surface_height` matches the extracted heightmap.
    #[test]
    fn live_generate_full_smoke() {
        let height = surface_height(8, 8); // surfaces the chunk via the global pipeline
        let chunk = generate_full(0, 0);
        assert_eq!(chunk.blocks.ocean_floor_wg[(8 * 16 + 8) as usize] - 1, height);
        assert_eq!(chunk.blocks.block(0, chunk.blocks.min_y, 0), ParityBlock::Bedrock);
        let top = chunk.blocks.min_y + chunk.blocks.height - 1;
        assert_eq!(chunk.blocks.block(8, top, 8), ParityBlock::Air);
        let count = with_pipeline(|p| p.generator.source.biome_count()) as u16;
        assert!(chunk.surface_biomes.iter().all(|&f| f < count));
    }

    #[test]
    fn status_names_round_trip() {
        for status in ChunkStatus::ALL {
            assert_eq!(ChunkStatus::from_name(status.name()), Some(status));
        }
        assert_eq!(ChunkStatus::from_name("full"), Some(Full));
        assert_eq!(ChunkStatus::from_name("minecraft:nonsense"), None);
        assert!(Full.is_or_after(Surface));
        assert!(!Biomes.is_or_after(Noise));
    }

    /// The staged pipeline produces the same terrain as the single-shot P5
    /// facade: same blocks and WG heightmaps after SURFACE.
    #[test]
    fn staged_pipeline_matches_single_shot() {
        let seed = 8000;
        let single = SurfacedGenerator::new_overworld(seed);
        let mut pipeline = ChunkPipeline::new_overworld(seed);
        for (cx, cz) in [(0, 0), (-3, 7)] {
            let expected = single.generate_chunk(cx, cz);
            let staged = pipeline.advance(cx, cz, Surface);
            let blocks = staged.blocks.as_ref().expect("surface chunk has blocks");
            assert_eq!(blocks.blocks, expected.blocks, "blocks differ at ({cx},{cz})");
            assert_eq!(blocks.ocean_floor_wg, expected.ocean_floor_wg);
            assert_eq!(blocks.world_surface_wg, expected.world_surface_wg);
        }
    }

    /// CARVERS (P7): advancing past SURFACE carves the terrain, deterministically,
    /// and only ever opens carver-replaceable blocks into air/water/lava. There
    /// is no JVM harness here, so this is a self-consistency check rather than a
    /// golden diff: carving must (a) reproduce byte-for-byte across independent
    /// instances, (b) leave the bedrock floor intact, (c) never touch the
    /// protected top 7 blocks of the build height, and (d) actually change the
    /// terrain somewhere across a small region.
    #[test]
    fn carvers_carve_terrain_deterministically() {
        let seed = 8000;
        let single = SurfacedGenerator::new_overworld(seed);
        let mut a = ChunkPipeline::new_overworld(seed);
        let mut b = ChunkPipeline::new_overworld(seed);
        // Warm b with an unrelated chunk so its biome-source last-result state
        // and proto cache differ when it reaches the targets.
        b.advance(-9, 4, Carvers);

        let mut total_carved = 0usize;
        for (cx, cz) in [(0, 0), (1, 0), (0, 1), (3, -2)] {
            let surface = single.generate_chunk(cx, cz);

            let carved = {
                let p = a.advance(cx, cz, Carvers);
                p.blocks.clone().unwrap()
            };
            let carved_b = {
                let p = b.advance(cx, cz, Carvers);
                p.blocks.clone().unwrap()
            };
            assert_eq!(carved.blocks, carved_b.blocks, "carvers nondeterministic at ({cx},{cz})");

            // Bedrock floor survives; the top of the build height is protected.
            assert_eq!(carved.block(0, carved.min_y, 0), ParityBlock::Bedrock);
            let top = carved.min_y + carved.height - 1;
            for lz in 0..16 {
                for lx in 0..16 {
                    for dy in 0..7 {
                        assert_eq!(
                            carved.block(lx, top - dy, lz),
                            surface.block(lx, top - dy, lz),
                            "carved into the protected top band at ({cx},{cz})",
                        );
                    }
                }
            }

            // Every changed block is an aquifer carve state (air/water/lava).
            for (s, c) in surface.blocks.iter().zip(carved.blocks.iter()) {
                if s != c {
                    total_carved += 1;
                    assert!(
                        matches!(c, ParityBlock::Air | ParityBlock::Water | ParityBlock::Lava),
                        "carve produced a non-air/water/lava block: {c:?}",
                    );
                }
            }

            // Idempotent: re-advancing to CARVERS does not carve again.
            let again = {
                let p = a.advance(cx, cz, Carvers);
                p.blocks.clone().unwrap()
            };
            assert_eq!(again.blocks, carved.blocks, "CARVERS not idempotent at ({cx},{cz})");
        }
        assert!(total_carved > 0, "carvers changed nothing across the sample region");
    }

    /// Independent pipeline instances of the same seed produce byte-identical
    /// chunks — the invariant the per-worker `thread_local!` pipelines rely on.
    /// Generating an unrelated chunk on one instance first (exercising the
    /// RTree's cross-chunk last-result state) must not perturb the target's
    /// blocks or biomes.
    #[test]
    fn independent_instances_generate_identically() {
        let seed = 8000;
        let (cx, cz) = (2, 5);

        let mut a = ChunkPipeline::new_overworld(seed);
        let mut b = ChunkPipeline::new_overworld(seed);
        // Warm `b` with a different chunk first, so its biome-source last-result
        // seed and proto cache differ from a's when it reaches the target.
        b.advance(-7, 3, Surface);

        a.advance(cx, cz, Surface);
        b.advance(cx, cz, Surface);

        let pa = a.chunk(cx, cz).unwrap();
        let pb = b.chunk(cx, cz).unwrap();
        let (ba, bb) = (pa.blocks.as_ref().unwrap(), pb.blocks.as_ref().unwrap());
        assert_eq!(ba.blocks, bb.blocks, "blocks differ across instances");
        assert_eq!(ba.ocean_floor_wg, bb.ocean_floor_wg, "OCEAN_FLOOR_WG differs");
        assert_eq!(ba.world_surface_wg, bb.world_surface_wg, "WORLD_SURFACE_WG differs");
        assert_eq!(
            pa.biome_sections.as_ref().unwrap(),
            pb.biome_sections.as_ref().unwrap(),
            "biomes differ across instances",
        );
    }

    /// Wall-clock sanity check that independent per-thread pipelines scale:
    /// generate 64 distinct chunks to SURFACE on 1 thread, then split across 4.
    /// Ignored (a benchmark, not an assertion) — run with
    /// `cargo test --release generation_scales_across_threads -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn generation_scales_across_threads() {
        use std::time::Instant;
        let seed = 8000;
        let coords: Vec<(i32, i32)> = (0..64).map(|i| (i % 8, i / 8)).collect();

        let t0 = Instant::now();
        {
            let mut p = ChunkPipeline::new_overworld(seed);
            for &(cx, cz) in &coords {
                p.advance(cx, cz, Surface);
            }
        }
        let single = t0.elapsed();

        let t1 = Instant::now();
        let threads = 4;
        std::thread::scope(|s| {
            for t in 0..threads {
                let mine: Vec<_> =
                    coords.iter().copied().skip(t).step_by(threads).collect();
                s.spawn(move || {
                    let mut p = ChunkPipeline::new_overworld(seed);
                    for (cx, cz) in mine {
                        p.advance(cx, cz, Surface);
                    }
                });
            }
        });
        let parallel = t1.elapsed();

        println!(
            "64 chunks to SURFACE: 1 thread {single:?}, {threads} threads {parallel:?} \
             ({:.2}x)",
            single.as_secs_f64() / parallel.as_secs_f64()
        );
    }

    /// Advancing to FULL walks the pyramid: the center reaches FULL and every
    /// neighbor ring reaches at least its accumulated requirement.
    #[test]
    fn full_advance_generates_the_dependency_pyramid() {
        let mut pipeline = ChunkPipeline::new_overworld(0);
        pipeline.advance(0, 0, Full);
        assert_eq!(pipeline.chunk(0, 0).unwrap().status, Full);
        let full = ChunkPyramid::generation().step_to(Full);
        for distance in 1..=full.accumulated_dependencies.radius() {
            let required = full.accumulated_dependencies.get(distance);
            for (cx, cz) in [(distance, 0), (0, -distance), (-distance, distance)] {
                let status = pipeline.chunk(cx, cz).expect("dependency generated").status;
                assert!(
                    status.is_or_after(required),
                    "chunk ({cx},{cz}) at distance {distance} is {status:?}, needs {required:?}"
                );
            }
        }
        // Advancing again is a no-op (idempotent).
        pipeline.advance(0, 0, Full);
        assert_eq!(pipeline.chunk(0, 0).unwrap().status, Full);
    }

    /// The region enforces vanilla read/write bounds: reads reach the
    /// dependency radius, writes only the step's write radius.
    #[test]
    fn region_enforces_read_and_write_bounds() {
        let mut pipeline = ChunkPipeline::new_overworld(0);
        // Surface both the center and a neighbor so both hold blocks.
        pipeline.advance(0, 0, Surface);
        pipeline.advance(1, 0, Surface);

        let pyramid = ChunkPyramid::generation();
        // FEATURES: write radius 1 — the neighbor write lands.
        let mut region = pipeline.region_for((0, 0), pyramid.step_to(Features));
        assert!(region.has_chunk(1, 0));
        assert!(!region.has_chunk(9, 0), "reads stop at the dependency radius (8)");
        assert!(region.set_block(20, 100, 4, ParityBlock::Stone));
        assert_eq!(region.get_block(20, 100, 4), ParityBlock::Stone);
        // Out-of-range y is dropped, not panicked.
        assert!(!region.set_block(20, -1000, 4, ParityBlock::Stone));

        // NOISE: write radius 0 — a cross-chunk write is dropped.
        let mut region = pipeline.region_for((0, 0), pyramid.step_to(Noise));
        assert!(!region.set_block(20, 100, 4, ParityBlock::Air));
        assert!(region.set_block(4, 100, 4, ParityBlock::Stone));
        assert_eq!(region.get_block(4, 100, 4), ParityBlock::Stone);

        // Biome reads route to the owning chunk's stored sections and match
        // the biome source directly.
        let region = pipeline.region_for((0, 0), pyramid.step_to(Features));
        let via_region = region.get_noise_biome(1, 10, 2);
        drop(region);
        let sections = pipeline.chunk(0, 0).unwrap().biome_sections.as_ref().unwrap();
        let min_section_y = pipeline.generator.inner.random_state.settings.noise.min_y >> 4;
        let section = &sections[((10 >> 2) - min_section_y) as usize];
        assert_eq!(via_region, section[(((10 & 3) * 4 + 2) * 4 + 1) as usize]);
    }
}
