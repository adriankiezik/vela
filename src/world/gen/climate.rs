//! Vanilla-parity climate space + biome source (P4).
//!
//! Ports `world/level/biome/Climate.java` exactly: 10000× quantized climate
//! coordinates, 7-dimensional `ParameterPoint`s (temperature, humidity,
//! continentalness, erosion, depth, weirdness, offset), and the `RTree`
//! nearest-neighbor index — including its branching factor of 6, the
//! best-split build (whose sort/bucket order affects tie-breaking, so it is
//! transcribed rather than substituted), and the last-result-seeded search.
//! On top sit `Climate.Sampler` (the six router climate functions evaluated
//! at quart resolution) and `MultiNoiseBiomeSource` driven by the expanded
//! overworld parameter list dumped from the official data generator, plus
//! `fillBiomesFromNoise`'s per-section 4×4×4 quart fill.
//!
//! `Climate.SpawnFinder` (vanilla spawn-point selection) is deferred until
//! the parity generator drives the live chunk path.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use super::density::{compute_at, compute_at_with, ChunkEvalState, Dfn, RandomState};
use super::vanilla_jsons;

/// `Climate.quantizeCoord` — float climate coordinate to fixed-point long.
/// The f32 multiply then truncating cast replicate Java's
/// `(long)(coord * 10000.0F)` bit-for-bit.
pub fn quantize_coord(coord: f32) -> i64 {
    (coord * 10000.0f32) as i64
}

/// `Climate.Parameter` — a quantized `[min, max]` interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub min: i64,
    pub max: i64,
}

impl Parameter {
    /// `Parameter.distance(long)` — 0 inside the interval, else the gap.
    fn distance(self, target: i64) -> i64 {
        let above = target - self.max;
        let below = self.min - target;
        if above > 0 {
            above
        } else {
            below.max(0)
        }
    }

    /// `Parameter.span(@Nullable Parameter)` — the enclosing interval.
    fn span(self, other: Option<Parameter>) -> Parameter {
        match other {
            None => self,
            Some(o) => Parameter { min: self.min.min(o.min), max: self.max.max(o.max) },
        }
    }
}

/// `Climate.TargetPoint` in its `toParameterArray()` form: the six sampled
/// climate coordinates plus the constant 0 the offset dimension is matched
/// against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetPoint(pub [i64; 7]);

impl TargetPoint {
    pub fn new(
        temperature: i64,
        humidity: i64,
        continentalness: i64,
        erosion: i64,
        depth: i64,
        weirdness: i64,
    ) -> Self {
        Self([temperature, humidity, continentalness, erosion, depth, weirdness, 0])
    }
}

/// A `ParameterPoint` in its 7-interval `parameterSpace()` form (the offset
/// becomes the degenerate interval `[offset, offset]`, matched against 0 —
/// which reproduces `fitness`'s `+ square(offset)` term since offsets are
/// non-negative).
type ParameterSpace = [Parameter; 7];

/// `ParameterPoint.fitness` / `RTree.Node.distance`: squared distance summed
/// over the 7 dimensions.
fn fitness(space: &ParameterSpace, target: &[i64; 7]) -> i64 {
    let mut total = 0i64;
    for i in 0..7 {
        let d = space[i].distance(target[i]);
        total += d * d;
    }
    total
}

// ---------------------------------------------------------------------------
// RTree (Climate.RTree)
// ---------------------------------------------------------------------------

/// One arena node of the search tree. Java compares nodes by reference
/// identity during search (`child == leaf`); arena indices stand in for that.
struct Node {
    bounds: ParameterSpace,
    kind: NodeKind,
}

enum NodeKind {
    /// Index into [`ParameterList::values`].
    Leaf(u32),
    Sub(Vec<u32>),
}

/// `Climate.ParameterList` + its `RTree`: the input entries (kept for the
/// brute-force cross-check) and the built index. Values are `u16` handles the
/// caller maps to biomes.
pub struct ParameterList {
    values: Vec<(ParameterSpace, u16)>,
    nodes: Vec<Node>,
    root: u32,
    /// Vanilla's `ThreadLocal<Leaf> lastResult` — the previous search's leaf
    /// seeds the next search's pruning bound. Only equal-distance ties can
    /// observe it (a tie keeps the incumbent), matching vanilla's per-thread
    /// reuse when queries run in the same order.
    last_result: Cell<Option<u32>>,
}

/// A node under construction: children owned inline, mirroring the recursive
/// Java build before flattening into the arena.
struct BuildNode {
    bounds: ParameterSpace,
    kind: BuildKind,
}

enum BuildKind {
    Leaf(u32),
    Sub(Vec<BuildNode>),
}

/// `(parameter.min + parameter.max) / 2` with Java's truncating division.
fn center(p: Parameter) -> i64 {
    (p.min + p.max) / 2
}

/// `RTree.buildParameterSpace` — per-dimension span over the children.
fn span_over(children: &[BuildNode]) -> ParameterSpace {
    let mut bounds: [Option<Parameter>; 7] = [None; 7];
    for child in children {
        for d in 0..7 {
            bounds[d] = Some(child.bounds[d].span(bounds[d]));
        }
    }
    bounds.map(|b| b.expect("SubTree needs at least one child"))
}

/// The sort key of `RTree.sort(…, dimension, absolute)`: interval centers
/// over all 7 dimensions starting at `dimension` and wrapping, optionally by
/// magnitude. Lexicographic comparison of the key reproduces the chained
/// comparators; the sorts are stable on both sides (TimSort / Rust sort).
fn sort_key(bounds: &ParameterSpace, dimension: usize, absolute: bool) -> [i64; 7] {
    let mut key = [0i64; 7];
    for d in 0..7 {
        let c = center(bounds[(dimension + d) % 7]);
        key[d] = if absolute { c.abs() } else { c };
    }
    key
}

/// One bucket of a candidate split: its span bounds plus the member indices
/// (into the `children` list) in their sorted order.
struct Bucket {
    bounds: ParameterSpace,
    members: Vec<usize>,
}

/// `RTree.bucketize` over a pre-sorted index order: chunks of
/// `6^floor(log6(n − 0.01))` children, plus a remainder bucket.
fn bucketize(children: &[BuildNode], order: &[usize]) -> Vec<Bucket> {
    let n = order.len();
    let expected = 6f64.powf(((n as f64 - 0.01).ln() / 6f64.ln()).floor()) as usize;
    let mut buckets = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for &i in order {
        current.push(i);
        if current.len() >= expected {
            buckets.push(bucket_of(children, std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        buckets.push(bucket_of(children, current));
    }
    buckets
}

fn bucket_of(children: &[BuildNode], members: Vec<usize>) -> Bucket {
    let mut bounds: [Option<Parameter>; 7] = [None; 7];
    for &i in &members {
        for d in 0..7 {
            bounds[d] = Some(children[i].bounds[d].span(bounds[d]));
        }
    }
    Bucket { bounds: bounds.map(|b| b.expect("bucket has members")), members }
}

/// `RTree.build` — the recursive best-split construction. Sort order,
/// bucketing, cost comparison (first-minimum wins), and the final
/// absolute-centered bucket sort are all transcribed: they determine tree
/// shape and hence search tie-breaking.
fn build(mut children: Vec<BuildNode>) -> BuildNode {
    assert!(!children.is_empty(), "need at least one child to build a node");
    if children.len() == 1 {
        return children.pop().expect("len checked");
    }
    if children.len() <= 6 {
        // Small node: order children by total center magnitude.
        children
            .sort_by_key(|child| (0..7).map(|d| center(child.bounds[d]).abs()).sum::<i64>());
        let bounds = span_over(&children);
        return BuildNode { bounds, kind: BuildKind::Sub(children) };
    }

    // Try bucketizing along every dimension; keep the cheapest split
    // (vanilla's strict `minCost > totalCost` keeps the first minimum).
    let mut min_cost = i64::MAX;
    let mut min_dimension = 0usize;
    let mut min_buckets: Vec<Bucket> = Vec::new();
    for d in 0..7 {
        // `sort(children, …, d, false)` as a stable index sort — buckets
        // remember member order, so mutating the shared list is unnecessary.
        let mut order: Vec<usize> = (0..children.len()).collect();
        order.sort_by_key(|&i| sort_key(&children[i].bounds, d, false));
        let buckets = bucketize(&children, &order);
        let total_cost: i64 = buckets
            .iter()
            .map(|b| b.bounds.iter().map(|p| (p.max - p.min).abs()).sum::<i64>())
            .sum();
        if min_cost > total_cost {
            min_cost = total_cost;
            min_dimension = d;
            min_buckets = buckets;
        }
    }

    // `sort(minBuckets, …, minDimension, true)` over the bucket bounds.
    min_buckets.sort_by_key(|b| sort_key(&b.bounds, min_dimension, true));

    // Recurse into each bucket. The buckets partition `children`, so each
    // child moves into exactly one recursive call.
    let mut slots: Vec<Option<BuildNode>> = children.into_iter().map(Some).collect();
    let built: Vec<BuildNode> = min_buckets
        .into_iter()
        .map(|bucket| {
            let members: Vec<BuildNode> = bucket
                .members
                .into_iter()
                .map(|i| slots[i].take().expect("bucket members are disjoint"))
                .collect();
            build(members)
        })
        .collect();
    let bounds = span_over(&built);
    BuildNode { bounds, kind: BuildKind::Sub(built) }
}

impl ParameterList {
    /// `Climate.ParameterList::new` + `RTree.create`: entries in registry
    /// order (order is the RTree build input order — load-bearing).
    pub fn new(values: Vec<(ParameterSpace, u16)>) -> Self {
        assert!(!values.is_empty(), "need at least one value to build the search tree");
        let leaves: Vec<BuildNode> = values
            .iter()
            .enumerate()
            .map(|(i, (space, _))| BuildNode { bounds: *space, kind: BuildKind::Leaf(i as u32) })
            .collect();
        let root_build = build(leaves);
        let mut nodes = Vec::new();
        let root = flatten(root_build, &mut nodes);
        Self { values, nodes, root, last_result: Cell::new(None) }
    }

    fn node_distance(&self, node: u32, target: &[i64; 7]) -> i64 {
        fitness(&self.nodes[node as usize].bounds, target)
    }

    /// `RTree.Node.search` — depth-first with the incumbent-leaf pruning
    /// bound. Strict `>` comparisons mean ties keep the incumbent, exactly
    /// as vanilla.
    fn search_node(&self, node: u32, target: &[i64; 7], candidate: Option<u32>) -> u32 {
        let children = match &self.nodes[node as usize].kind {
            NodeKind::Leaf(_) => return node,
            NodeKind::Sub(children) => children,
        };
        let mut min_distance =
            candidate.map_or(i64::MAX, |c| self.node_distance(c, target));
        let mut closest = candidate;
        for &child in children {
            let child_distance = self.node_distance(child, target);
            if min_distance > child_distance {
                let leaf = self.search_node(child, target, closest);
                let leaf_distance = if child == leaf {
                    child_distance
                } else {
                    self.node_distance(leaf, target)
                };
                if min_distance > leaf_distance {
                    min_distance = leaf_distance;
                    closest = Some(leaf);
                }
            }
        }
        closest.expect("subtree has at least one child")
    }

    /// `ParameterList.findValue` — the indexed nearest-neighbor lookup.
    pub fn find_value(&self, target: TargetPoint) -> u16 {
        let leaf = self.search_node(self.root, &target.0, self.last_result.get());
        self.last_result.set(Some(leaf));
        match self.nodes[leaf as usize].kind {
            NodeKind::Leaf(value) => self.values[value as usize].1,
            NodeKind::Sub(_) => unreachable!("search returns a leaf"),
        }
    }

    /// `ParameterList.findValueBruteForce` — linear scan, for cross-checks.
    #[cfg(test)]
    pub fn find_value_brute_force(&self, target: TargetPoint) -> (i64, u16) {
        let mut best = (fitness(&self.values[0].0, &target.0), self.values[0].1);
        for (space, value) in &self.values[1..] {
            let f = fitness(space, &target.0);
            if f < best.0 {
                best = (f, *value);
            }
        }
        best
    }

    /// The fitness of the entry a search returned would have — used to
    /// verify the RTree result is a true nearest neighbor.
    #[cfg(test)]
    fn best_fitness_of(&self, target: TargetPoint) -> i64 {
        let leaf = self.search_node(self.root, &target.0, self.last_result.get());
        self.last_result.set(Some(leaf));
        fitness(&self.nodes[leaf as usize].bounds, &target.0)
    }
}

/// Flatten the built tree into the arena, returning the node's index.
fn flatten(node: BuildNode, nodes: &mut Vec<Node>) -> u32 {
    let kind = match node.kind {
        BuildKind::Leaf(value) => NodeKind::Leaf(value),
        BuildKind::Sub(children) => {
            NodeKind::Sub(children.into_iter().map(|c| flatten(c, nodes)).collect())
        }
    };
    nodes.push(Node { bounds: node.bounds, kind });
    (nodes.len() - 1) as u32
}

// ---------------------------------------------------------------------------
// Climate.Sampler
// ---------------------------------------------------------------------------

/// `Climate.Sampler`: the router's six climate functions sampled at quart
/// resolution with a `SinglePointContext`. Vanilla flattens cache markers
/// out of these graphs first (`noiseFlattener`), but markers delegate to
/// their wrapped function under point evaluation, so [`compute_at`] on the
/// router graphs is bit-identical.
pub struct Sampler {
    temperature: Rc<Dfn>,
    humidity: Rc<Dfn>,
    continentalness: Rc<Dfn>,
    erosion: Rc<Dfn>,
    depth: Rc<Dfn>,
    weirdness: Rc<Dfn>,
}

/// The five quantized climate coordinates of a quart column that do not vary
/// with Y — everything a `TargetPoint` needs except `depth`. Cached once per
/// column during a biome fill (vanilla's `FlatCache`). The `Default` (all-zero)
/// value only backs the fill array's initialisation; every entry is overwritten
/// before it is read.
#[derive(Clone, Copy, Default)]
struct ColumnClimate {
    temperature: i64,
    humidity: i64,
    continentalness: i64,
    erosion: i64,
    weirdness: i64,
}

impl Sampler {
    /// The wiring in `RandomState`: humidity is the router's `vegetation`,
    /// weirdness its `ridges`.
    pub fn new(rs: &RandomState) -> Self {
        Self {
            temperature: rs.router.temperature.clone(),
            humidity: rs.router.vegetation.clone(),
            continentalness: rs.router.continents.clone(),
            erosion: rs.router.erosion.clone(),
            depth: rs.router.depth.clone(),
            weirdness: rs.router.ridges.clone(),
        }
    }

    /// `Sampler.sample(quartX, quartY, quartZ)` — evaluate at the quart's
    /// block position and quantize each coordinate through f32.
    pub fn sample(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> TargetPoint {
        let (x, y, z) = (quart_x << 2, quart_y << 2, quart_z << 2);
        let q = |f: &Rc<Dfn>| quantize_coord(compute_at(f, x, y, z) as f32);
        TargetPoint::new(
            q(&self.temperature),
            q(&self.humidity),
            q(&self.continentalness),
            q(&self.erosion),
            q(&self.depth),
            q(&self.weirdness),
        )
    }

    /// Sample the five 2-D climate coordinates that only depend on `(x, z)` for
    /// one quart column, reusing `st`. Vanilla's `cachedClimateSampler`
    /// FlatCache-wraps these functions and prefills them at `y = 0`; here the
    /// fillers are genuinely two-dimensional (temperature/humidity are
    /// `shiftedNoise2d`, whose `yScale` is 0 and whose shift noises hardcode
    /// `y = 0`; continentalness/erosion/weirdness are `flatCache` over the same),
    /// so the value is identical at every `y`. Computed once per column and
    /// reused across all 96 quart-Y levels of the chunk.
    fn sample_column(&self, quart_x: i32, quart_z: i32, st: &mut ChunkEvalState) -> ColumnClimate {
        let (x, z) = (quart_x << 2, quart_z << 2);
        // Mirror FlatCache's prefill position (`y = 0`); value is y-invariant.
        let q = |f: &Rc<Dfn>, st: &mut ChunkEvalState| {
            quantize_coord(compute_at_with(f, x, 0, z, st) as f32)
        };
        ColumnClimate {
            temperature: q(&self.temperature, st),
            humidity: q(&self.humidity, st),
            continentalness: q(&self.continentalness, st),
            erosion: q(&self.erosion, st),
            weirdness: q(&self.weirdness, st),
        }
    }

    /// `Sampler.sample` for one quart using a column's cached 2-D coordinates —
    /// only `depth` (the sole Y-dependent climate function) is evaluated here.
    fn sample_with_column(
        &self,
        col: &ColumnClimate,
        quart_x: i32,
        quart_y: i32,
        quart_z: i32,
        st: &mut ChunkEvalState,
    ) -> TargetPoint {
        let (x, y, z) = (quart_x << 2, quart_y << 2, quart_z << 2);
        let depth = quantize_coord(compute_at_with(&self.depth, x, y, z, st) as f32);
        TargetPoint::new(
            col.temperature,
            col.humidity,
            col.continentalness,
            col.erosion,
            depth,
            col.weirdness,
        )
    }
}

// ---------------------------------------------------------------------------
// MultiNoiseBiomeSource
// ---------------------------------------------------------------------------

/// `MultiNoiseBiomeSource` over the expanded overworld parameter list: a
/// deduplicated biome-id table plus the climate index.
pub struct MultiNoiseBiomeSource {
    list: ParameterList,
    biomes: Vec<String>,
}

impl MultiNoiseBiomeSource {
    /// The `minecraft:overworld` preset, from the vendored data-generator
    /// dump (entry order preserved — it drives RTree construction).
    pub fn overworld() -> Self {
        let v: Value = serde_json::from_str(vanilla_jsons::OVERWORLD_BIOME_PARAMETERS)
            .expect("bad biome parameter JSON");
        let mut biomes: Vec<String> = Vec::new();
        let mut ids: HashMap<String, u16> = HashMap::new();
        let entries = v["biomes"].as_array().expect("biomes array");
        let mut values = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = entry["biome"].as_str().expect("biome id");
            let id = *ids.entry(name.to_owned()).or_insert_with(|| {
                biomes.push(name.to_owned());
                (biomes.len() - 1) as u16
            });
            let p = &entry["parameters"];
            let param = |field: &str| -> Parameter {
                let f = &p[field];
                // The codec parses floats; a bare number is a point interval.
                if let Some(pair) = f.as_array() {
                    Parameter {
                        min: quantize_coord(pair[0].as_f64().expect("min") as f32),
                        max: quantize_coord(pair[1].as_f64().expect("max") as f32),
                    }
                } else {
                    let q = quantize_coord(f.as_f64().expect("point") as f32);
                    Parameter { min: q, max: q }
                }
            };
            let offset = quantize_coord(p["offset"].as_f64().expect("offset") as f32);
            values.push((
                [
                    param("temperature"),
                    param("humidity"),
                    param("continentalness"),
                    param("erosion"),
                    param("depth"),
                    param("weirdness"),
                    Parameter { min: offset, max: offset },
                ],
                id,
            ));
        }
        Self { list: ParameterList::new(values), biomes }
    }

    /// `MultiNoiseBiomeSource.getNoiseBiome(x, y, z, sampler)` — the biome
    /// registry id at a quart position.
    pub fn get_noise_biome(
        &self,
        quart_x: i32,
        quart_y: i32,
        quart_z: i32,
        sampler: &Sampler,
    ) -> &str {
        &self.biomes[self.list.find_value(sampler.sample(quart_x, quart_y, quart_z)) as usize]
    }

    /// `ChunkAccess.fillBiomesFromNoise` for one chunk: for each 16-block
    /// section bottom-up, the 4×4×4 quart biomes, stored at the vanilla
    /// container index `(y·4 + z)·4 + x` and visited in vanilla's
    /// `LevelChunkSection` loop order (x, then y, then z) — visit order
    /// matters because the RTree search is stateful on ties.
    pub fn fill_chunk_biomes(
        &self,
        sampler: &Sampler,
        chunk_x: i32,
        chunk_z: i32,
        min_y: i32,
        height: i32,
    ) -> Vec<[u16; 64]> {
        let quart_min_x = (chunk_x * 16) >> 2;
        let quart_min_z = (chunk_z * 16) >> 2;
        let min_section_y = min_y >> 4;
        let section_count = height / 16;
        let mut sections = Vec::with_capacity(section_count as usize);

        // One evaluation state for every density-function sample of the chunk
        // (vanilla's per-`NoiseChunk` sampler), plus a per-column cache of the
        // Y-invariant climate coordinates (vanilla's `FlatCache`): the chunk's
        // 4×4 quart columns are sampled once here and reused across all
        // `section_count · 4` quart-Y levels, so only `depth` is evaluated per
        // quart. This changes neither the values nor the `find_value` call order
        // below, so the stateful RTree search is bit-identical.
        let mut st = ChunkEvalState::standalone();
        let mut columns = [ColumnClimate::default(); 16];
        for x in 0..4 {
            for z in 0..4 {
                columns[(x * 4 + z) as usize] =
                    sampler.sample_column(quart_min_x + x, quart_min_z + z, &mut st);
            }
        }

        for section_index in 0..section_count {
            let quart_min_y = (min_section_y + section_index) << 2;
            let mut section = [0u16; 64];
            for x in 0..4 {
                for y in 0..4 {
                    for z in 0..4 {
                        let col = &columns[(x * 4 + z) as usize];
                        let target = sampler.sample_with_column(
                            col,
                            quart_min_x + x,
                            quart_min_y + y,
                            quart_min_z + z,
                            &mut st,
                        );
                        section[((y * 4 + z) * 4 + x) as usize] = self.list.find_value(target);
                    }
                }
            }
            sections.push(section);
        }
        sections
    }

    /// The biome registry id for a fill value.
    pub fn biome_name(&self, id: u16) -> &str {
        &self.biomes[id as usize]
    }

    /// The fill value for a biome registry id (`minecraft:`-prefixed), if the
    /// biome appears in the parameter list.
    pub fn biome_index(&self, name: &str) -> Option<u16> {
        self.biomes.iter().position(|b| b == name).map(|i| i as u16)
    }

    /// The number of distinct biomes in the parameter list.
    pub fn biome_count(&self) -> usize {
        self.biomes.len()
    }
}

// ---------------------------------------------------------------------------
// Climate.SpawnFinder
// ---------------------------------------------------------------------------

/// The overworld `spawn_target` climate points, from the vendored
/// `noise_settings/overworld.json` (`OverworldBiomeBuilder.spawnTarget()`
/// serialized). Each becomes a 7-interval parameter space — the offset is the
/// degenerate `[offset, offset]` interval matched against 0, exactly as
/// `ParameterPoint.parameterSpace()`/`fitness` treat it. The `depth` interval
/// is `[0, 0]`, matching the zero-depth target the finder samples against.
fn spawn_target() -> Vec<ParameterSpace> {
    let v: Value = serde_json::from_str(vanilla_jsons::OVERWORLD_NOISE_SETTINGS)
        .expect("bad noise settings JSON");
    let entries = v["spawn_target"].as_array().expect("spawn_target array");
    entries
        .iter()
        .map(|e| {
            let param = |field: &str| -> Parameter {
                let f = &e[field];
                if let Some(pair) = f.as_array() {
                    Parameter {
                        min: quantize_coord(pair[0].as_f64().expect("min") as f32),
                        max: quantize_coord(pair[1].as_f64().expect("max") as f32),
                    }
                } else {
                    let q = quantize_coord(f.as_f64().expect("point") as f32);
                    Parameter { min: q, max: q }
                }
            };
            let offset = quantize_coord(e["offset"].as_f64().expect("offset") as f32);
            [
                param("temperature"),
                param("humidity"),
                param("continentalness"),
                param("erosion"),
                param("depth"),
                param("weirdness"),
                Parameter { min: offset, max: offset },
            ]
        })
        .collect()
}

/// `Mth.square(2048L)` — the fitness weight that dominates the origin distance
/// bias in `SpawnFinder.getSpawnPositionAndFitness`.
const SPAWN_RADIUS_SQ: i64 = 2048 * 2048;

/// `SpawnFinder.getSpawnPositionAndFitness` — the climate fitness at a block
/// column, biased toward the world origin. Lower is better. The climate is
/// sampled at depth 0 (`zeroDepthTargetPoint`), then the minimum fitness over
/// the spawn-target points is scaled by `2048²` and the squared origin distance
/// is added.
fn spawn_fitness(targets: &[ParameterSpace], sampler: &Sampler, block_x: i32, block_z: i32) -> i64 {
    // `sampler.sample(QuartPos.fromBlock(x), 0, QuartPos.fromBlock(z))`.
    let mut target = sampler.sample(block_x >> 2, 0, block_z >> 2).0;
    target[4] = 0; // zero the depth dimension
    let mut min_fitness = i64::MAX;
    for space in targets {
        min_fitness = min_fitness.min(fitness(space, &target));
    }
    let dx = block_x as i64;
    let dz = block_z as i64;
    let distance_bias = dx * dx + dz * dz;
    min_fitness * SPAWN_RADIUS_SQ + distance_bias
}

/// `SpawnFinder.radialSearch` — sweep an Archimedean spiral of increasing
/// radius around a fixed origin, keeping the lowest-fitness column found. The
/// float arithmetic (and the `(int)(Math.sin(angle) * radius)` truncation, with
/// `Math.sin` promoting the `float` angle to `double`) is reproduced exactly.
#[allow(clippy::too_many_arguments)]
fn radial_search(
    targets: &[ParameterSpace],
    sampler: &Sampler,
    best_x: &mut i32,
    best_z: &mut i32,
    best_fitness: &mut i64,
    max_radius: f32,
    radius_increment: f32,
) {
    let mut angle: f32 = 0.0;
    let mut radius: f32 = radius_increment;
    // `searchOrigin = this.result.location()` — captured once for the sweep.
    let (origin_x, origin_z) = (*best_x, *best_z);
    while radius <= max_radius {
        let x = origin_x + (f64::sin(angle as f64) * radius as f64) as i32;
        let z = origin_z + (f64::cos(angle as f64) * radius as f64) as i32;
        let f = spawn_fitness(targets, sampler, x, z);
        if f < *best_fitness {
            *best_fitness = f;
            *best_x = x;
            *best_z = z;
        }
        angle += radius_increment / radius;
        if (angle as f64) > std::f64::consts::PI * 2.0 {
            angle = 0.0;
            radius += radius_increment;
        }
    }
}

/// `Climate.findSpawnPosition` / `Climate.SpawnFinder` — the coarse-to-fine
/// climate-space spawn search. Starts at the origin, then two radial sweeps
/// (2048→512, then 512→32), returning the block `(x, z)` of the best column.
/// This is what steers the world spawn toward temperate, inland, non-river
/// climates instead of whatever sits at the origin.
pub fn find_spawn_position(sampler: &Sampler) -> (i32, i32) {
    let targets = spawn_target();
    let (mut best_x, mut best_z) = (0i32, 0i32);
    let mut best_fitness = spawn_fitness(&targets, sampler, 0, 0);
    radial_search(&targets, sampler, &mut best_x, &mut best_z, &mut best_fitness, 2048.0, 512.0);
    radial_search(&targets, sampler, &mut best_x, &mut best_z, &mut best_fitness, 512.0, 32.0);
    (best_x, best_z)
}

#[cfg(test)]
mod tests {
    use super::super::density::VanillaWorldgenData;
    use super::*;

    #[test]
    fn parameter_list_loads() {
        let source = MultiNoiseBiomeSource::overworld();
        // The expanded overworld table: 7594 points over the 53 multi-noise
        // overworld biomes (48 surface/cave biomes plus variants).
        assert_eq!(source.list.values.len(), 7594);
        assert!(source.biomes.len() >= 48, "got {} biomes", source.biomes.len());
        assert!(source.biomes.iter().any(|b| b == "minecraft:deep_dark"));
        assert!(source.biomes.iter().any(|b| b == "minecraft:mushroom_fields"));
    }

    /// The RTree must return a true nearest neighbor (equal fitness to the
    /// brute-force scan) across a spread of targets — this checks the tree
    /// build independently of the JVM fixture.
    #[test]
    fn rtree_matches_brute_force() {
        let source = MultiNoiseBiomeSource::overworld();
        let mut checked = 0;
        for t in (-15000..=15000).step_by(7500) {
            for c in (-12000..=12000).step_by(6000) {
                for e in (-9000..=9000).step_by(4500) {
                    for w in [-8000i64, 0, 9000] {
                        for d in [0i64, 5000, 10000] {
                            let target = TargetPoint::new(t, -t / 2, c, e, d, w);
                            let (brute_fitness, _) = source.list.find_value_brute_force(target);
                            let tree_fitness = source.list.best_fitness_of(target);
                            assert_eq!(
                                tree_fitness, brute_fitness,
                                "fitness mismatch at {target:?}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 5 * 5 * 5 * 3 * 3);
    }

    /// Bit-for-bit parity against the reference JVM (`VelaP4Harness` on the
    /// real 26.2 server classes): sampled climate target points, biome
    /// lookups over wide grids, and per-chunk `fillBiomesFromNoise` digests,
    /// for 3 seeds.
    #[test]
    fn jvm_golden_parity_p4() {
        let fixture = include_str!("testdata/p4_golden.txt");
        let source = MultiNoiseBiomeSource::overworld();
        let mut samplers: HashMap<i64, Sampler> = HashMap::new();
        let data = VanillaWorldgenData::load_overworld();
        let mut checked = 0usize;
        for line in fixture.lines() {
            let mut parts = line.split_whitespace();
            let tag = parts.next().expect("tag");
            let seed: i64 = parts.next().expect("seed").parse().expect("seed");
            samplers
                .entry(seed)
                .or_insert_with(|| Sampler::new(&RandomState::new_overworld(&data, seed)));
            let sampler = &samplers[&seed];
            match tag {
                "climate" => {
                    let qx: i32 = parts.next().unwrap().parse().unwrap();
                    let qy: i32 = parts.next().unwrap().parse().unwrap();
                    let qz: i32 = parts.next().unwrap().parse().unwrap();
                    let want: Vec<i64> =
                        (0..6).map(|_| parts.next().unwrap().parse().unwrap()).collect();
                    let got = sampler.sample(qx, qy, qz);
                    assert_eq!(
                        &got.0[..6],
                        &want[..],
                        "climate seed {seed} at ({qx},{qy},{qz})"
                    );
                }
                "biome" => {
                    let qx: i32 = parts.next().unwrap().parse().unwrap();
                    let qy: i32 = parts.next().unwrap().parse().unwrap();
                    let qz: i32 = parts.next().unwrap().parse().unwrap();
                    let want = parts.next().expect("biome id");
                    let got = source.get_noise_biome(qx, qy, qz, sampler);
                    assert_eq!(got, want, "biome seed {seed} at ({qx},{qy},{qz})");
                }
                "biomefill" => {
                    let cx: i32 = parts.next().unwrap().parse().unwrap();
                    let cz: i32 = parts.next().unwrap().parse().unwrap();
                    let digest = u64::from_str_radix(parts.next().expect("digest"), 16).unwrap();
                    let sections = source.fill_chunk_biomes(sampler, cx, cz, -64, 384);
                    // FNV-1a over the biome ids in section order, vanilla's
                    // fill visit order (x, y, z), one `name\n` per quart.
                    let mut h = 0xcbf29ce484222325u64;
                    for section in &sections {
                        for x in 0..4usize {
                            for y in 0..4usize {
                                for z in 0..4usize {
                                    let name =
                                        source.biome_name(section[(y * 4 + z) * 4 + x]);
                                    for b in name.bytes().chain(std::iter::once(b'\n')) {
                                        h ^= b as u64;
                                        h = h.wrapping_mul(0x100000001b3);
                                    }
                                }
                            }
                        }
                    }
                    assert_eq!(h, digest, "biomefill seed {seed} chunk ({cx},{cz})");
                }
                other => panic!("unknown fixture tag {other}"),
            }
            checked += 1;
        }
        assert_eq!(checked, 801, "fixture line count");
    }

    fn overworld_sampler(seed: i64) -> Sampler {
        let data = VanillaWorldgenData::load_overworld();
        Sampler::new(&RandomState::new_overworld(&data, seed))
    }

    #[test]
    fn spawn_target_loads_two_points() {
        // The vendored overworld spawn_target: two ParameterPoints differing only
        // in the weirdness band (the two non-river slices), depth pinned to 0.
        let targets = spawn_target();
        assert_eq!(targets.len(), 2);
        let q = quantize_coord;
        for space in &targets {
            assert_eq!(space[4], Parameter { min: 0, max: 0 }, "depth must be [0,0]");
            assert_eq!(space[6], Parameter { min: 0, max: 0 }, "offset must be [0,0]");
            // continentalness = span(inlandContinentalness.min, FULL_RANGE.max)
            // = [-0.11, 1.0]; temperature/humidity/erosion are FULL_RANGE.
            assert_eq!(space[2], Parameter { min: q(-0.11), max: q(1.0) });
            assert_eq!(space[0], Parameter { min: q(-1.0), max: q(1.0) });
        }
        // weirdness slices: [-1.0, -0.16] and [0.16, 1.0].
        assert_eq!(targets[0][5], Parameter { min: q(-1.0), max: q(-0.16) });
        assert_eq!(targets[1][5], Parameter { min: q(0.16), max: q(1.0) });
    }

    #[test]
    fn spawn_finder_is_deterministic() {
        // The search is a pure function of the seed's climate router.
        for seed in [0i64, 1, 0x5EED_C0DE] {
            let sampler = overworld_sampler(seed);
            let a = find_spawn_position(&sampler);
            let b = find_spawn_position(&sampler);
            assert_eq!(a, b, "spawn search must be deterministic for seed {seed}");
        }
    }

    #[test]
    fn spawn_finder_beats_the_origin() {
        // The finder only ever replaces the incumbent with a strictly better
        // column, so the chosen spot's fitness is <= the origin's, and for real
        // seeds it strictly improves (the origin is rarely the global optimum).
        let targets = spawn_target();
        let mut improved = 0;
        for seed in [0i64, 1, 42, 0x5EED_C0DE] {
            let sampler = overworld_sampler(seed);
            let (sx, sz) = find_spawn_position(&sampler);
            let origin_fitness = spawn_fitness(&targets, &sampler, 0, 0);
            let spawn_fitness_v = spawn_fitness(&targets, &sampler, sx, sz);
            assert!(
                spawn_fitness_v <= origin_fitness,
                "seed {seed}: spawn fitness {spawn_fitness_v} worse than origin {origin_fitness}"
            );
            if spawn_fitness_v < origin_fitness {
                improved += 1;
            }
        }
        assert!(improved > 0, "the finder should improve on the origin for some seed");
    }

    #[test]
    fn spawn_finder_stays_in_search_range() {
        // Both sweeps are bounded by MAX_RADIUS (2048) around the origin, so the
        // result never escapes ~[-2048, 2048] on either axis.
        for seed in [0i64, 7, 1234] {
            let sampler = overworld_sampler(seed);
            let (sx, sz) = find_spawn_position(&sampler);
            assert!(sx.abs() <= 2048 && sz.abs() <= 2048, "seed {seed}: ({sx},{sz}) out of range");
        }
    }
}
