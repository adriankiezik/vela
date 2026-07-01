# Worldgen 1:1 Parity — Gap Analysis

What Vela must implement so that, for a given seed, generated chunks are
block-for-block and biome-for-biome identical to a vanilla 26.2 server.
Reference decompile: `C:\Users\kiezi\mc-decompile\src-server`.

## Progress

| Milestone | Status | Where |
|---|---|---|
| **P0 — RNG foundation** | **done** | `src/world/gen/random.rs`: Xoroshiro128++, Stafford-mix13 seed upgrade, MD5 `seedFromHashOf`, `Mth.getSeed`, both `RandomSource` algorithms + positional factories, `WorldgenRandom` with all four seed setters. 13 golden tests vs JVM reference values. |
| **P1 — Noise primitives** | **done** | `src/world/gen/synth.rs`: `ImprovedNoise` (incl. smear/fudge variant), `PerlinNoise` (new + legacy init), `NormalNoise`, `BlendedNoise`. 6 golden tests vs JVM reference values at overworld parameters. |
| **P2 — Density engine + router + fill** | **done** | `src/world/gen/density.rs`: full density-function node graph (JSON-codec-driven from the vendored datapack under `data/minecraft/worldgen/`, embedded via `src/world/gen/vanilla_jsons.rs`), exact min/max propagation + constant folding, `CubicSpline` in f32, `RandomState` noise wiring, `NoiseChunk` cache wrappers (`Interpolated`/`FlatCache`/`Cache2D`/`CacheOnce`/`CacheAllInCell`) with counter/fill-array semantics, `doFill` with the disabled-aquifer filler. Golden fixture (`testdata/p2_golden.txt`, 390 checks from a JVM harness on the real 26.2 server classes): all 15 router outputs bit-exact at 8 positions × 3 seeds; full-chunk fill digests, columns, and preliminary surface levels exact for 2 chunks × 3 seeds. Terrain *shape* now matches vanilla seed-for-seed (with aquifers/veins off, pending P3). |
| **P3 — Aquifers + ore veins** | **done** | `src/world/gen/density.rs`: full `NoiseChunk` router wrap (all 15 outputs, vanilla `mapAll` order), `Aquifer.NoiseBasedAquifer` (16×12×16 grid cells with positional-random offsets, 3-nearest similarity blending, barrier pressure, floodedness/spread/lava fluid levels, deep-dark suppression), `OreVeinifier` (copper y 0..50 / deepslate-iron y −60..−8, granite/tuff filler, 2% raw-ore), `MaterialRuleList` chain in `getInterpolatedState`. Runs with the vanilla-true `aquifers_enabled`/`ore_veins_enabled`. New golden fixture (`testdata/p3_golden.txt`, 390 checks from `VelaP3Harness` on the real 26.2 server classes) bit-exact; the P2 shape-only fixture still passes via a flags-off generator. `shouldScheduleFluidUpdate` (fluid-tick scheduling, not block output) is deferred to the chunk-persistence layer. |
| **P4 — Climate/biomes (RTree, parameter list)** | **next** | |
| P5 — Surface rules | pending | |
| P6 — Staged chunk pipeline | pending | |
| P7 — Carvers | pending | |
| P8 — Features/decoration | pending | |
| P9 — Structures | pending | |

Golden values are produced by transcription harnesses run on a local JVM
(kept outside the repo); only the resulting constants live in the Rust tests.
Since P2 the harness compiles directly against the (unobfuscated) 26.2 server
classes and its dump is committed as a test fixture. End-to-end `.mca` diffing
against a real vanilla server becomes possible at P2 (see "Verification
strategy").

Since P2 the vanilla built-in datapack's worldgen JSON (dumped via the
official data generator, per "Data extraction" below) is vendored under
`data/minecraft/worldgen/` and embedded at compile time — the engine is
data-driven rather than a transcription of `NoiseRouterData`.

## Where we stand

Vela's current generator (`src/world/gen/`) is an intentional approximation
("a believable overworld, not a `NoiseBasedChunkGenerator` port"). Of the whole
stack, only two pieces are already faithful and reusable:

- **`JavaRandom`** (`src/world/gen/rng.rs`) — exact `java.util.Random` LCG,
  tested against known sequences, plus `setDecorationSeed`.
- Chunk plumbing that parity work sits on top of: `PalettedContainer`
  encoding, heightmaps, Anvil persistence.

Everything else — value-noise height field, 16-biome temp×humidity matrix,
ad-hoc surface/bedrock/cave rules, per-chunk-confined decoration, single-pass
monolithic generation — must be replaced. **This is a rewrite of
`src/world/gen/`, not an incremental fix**, because vanilla's output is the
product of one deeply interconnected pipeline: the same density-function graph
feeds terrain shape, biome placement, aquifers, ore veins, and surface depth.

A consequence worth stating up front: parity generation is **seed-incompatible
with existing Vela worlds** — the same `level.dat` seed will produce different
terrain, so old worlds effectively reset (player edits diff against a baseline
that no longer matches).

## Layer 0 — RNG & seeding (prerequisite for everything)

Vanilla worldgen randomness flows from one place; getting any bit of it wrong
desynchronizes everything downstream.

| Piece | Reference | Notes |
|---|---|---|
| **Xoroshiro128++** | `XoroshiroRandomSource`, `Xoroshiro128PlusPlus` | 128-bit state; `nextLong`, `nextInt(bound)`, `nextDouble`, `consumeCount`. |
| **Seed upgrade** | `RandomSupport.upgradeSeedTo128bit` | 64-bit world seed → 128-bit state (MD5-based mixing). |
| **Positional forking** | `forkPositional()`, `PositionalRandomFactory` | `.at(x,y,z)` (block-pos hash XOR) and `.fromHashOf("minecraft:...")` (name hash XOR). Every named noise is seeded via `fromHashOf(noise id)`. |
| **`WorldgenRandom` seed setters** | `setDecorationSeed`, `setFeatureSeed`, `setLargeFeatureSeed`, `setLargeFeatureWithSalt` | We have `setDecorationSeed` on the legacy LCG; the rest are missing, and modern decoration runs on Xoroshiro. |
| **`RandomState`** | `RandomState.java` (139 ln) | Per-world container: lazily instantiates the ~48 named `NormalNoise` instances, aquifer/ore forked factories, the seeded router, climate sampler, surface system. |

Small, self-contained, unit-testable against known vanilla sequences. Do this
first.

## Layer 1 — Noise primitives (`levelgen/synth/`)

Replace our hand-written value noise entirely:

- **`ImprovedNoise`** — classic Perlin: 256-entry shuffled permutation table +
  per-instance xo/yo/zo offsets drawn from the RNG (consumption order
  matters), 16-gradient dot products, smoothstep, `lerp3`. Includes the legacy
  `yScale`/`yFudge` behavior.
- **`PerlinNoise`** — octave stack with `firstOctave` + sparse `amplitudes`
  list; **new mode** seeds each octave via `fromHashOf("octave_N")` (legacy
  mode uses linear RNG progression — needed only for carver-era code paths);
  coordinate wrapping at ~3.35e7.
- **`NormalNoise`** — two `PerlinNoise` instances averaged;
  `INPUT_FACTOR = 1.0181268882175227`, `valueFactor` from
  `expectedDeviation`. This is the workhorse — all ~48 named noises are
  `NormalNoise`.
- **`BlendedNoise`** — the legacy 3D terrain composite (8-octave main
  selecting between two 16-octave limit noises); still the overworld's base 3D
  noise (xzScale 0.25, yScale 0.125, xzFactor 80, yFactor 160, smear 8).
- **`SimplexNoise`** — End islands only; defer with the End.

**Data dependency:** each named noise's `firstOctave`/`amplitudes` come from
the `worldgen/noise` registry (vanilla datapack JSON — extractable via the
official data generator, see "Data extraction" below).

## Layer 2 — Density-function engine + the overworld router

The heart of the whole system (`DensityFunctions.java`, 1397 ln;
`NoiseRouterData.java`, 572 ln; `NoiseChunk.java`, 851 ln).

1. **A `DensityFunction` node graph** with ~30 node types: constants,
   `YClampedGradient`, noise/shifted-noise sources, unary ops (`abs`,
   `square`, `cube`, `half_negative`, `quarter_negative`, `squeeze`,
   `invert`), binary ops (`add`/`mul`/`min`/`max` with constant folding),
   `RangeChoice`, `Clamp`, `Spline`, plus min/max-value propagation used for
   short-circuit evaluation.
2. **Cache/marker wrappers** with exact vanilla semantics — `Interpolated`,
   `FlatCache` (quart-resolution 2D), `Cache2D`, `CacheOnce`,
   `CacheAllInCell`, `BlendDensity` — resolved by `NoiseChunk.wrap()`. These
   are not optimizations you can skip: `FlatCache` sampling at quart
   resolution *changes output values* (biome-scale sampling), so caching
   semantics are part of the spec.
3. **`CubicSpline`** evaluation and the **`TerrainProvider`**
   offset/factor/jaggedness splines over (continents, erosion, ridges_folded,
   weirdness) — dozens of hardcoded knot constants that must be transcribed
   exactly.
4. **The overworld `NoiseRouter` graph** (15 output functions): shift noises →
   2D climate noises (temperature, vegetation, continents, erosion, ridge,
   `ridges_folded = peaksAndValleys(ridge)`) → spline-driven offset/factor/
   jaggedness → depth → `initial_density` → `sloped_cheese` (+ BlendedNoise)
   → noise caves (cheese / spaghetti 2D & 3D / noodle / entrances / pillars)
   → `slideOverworld` top/bottom easing → `final_density`
   (`interpolated(blend_density(...) * 0.64).squeeze()`, min with noodle).
   Key constants: `GLOBAL_OFFSET = -0.50375`, cheese target −0.703125,
   surface-density threshold 1.5625, etc.
5. **`NoiseChunk`** — the stateful cell interpolator: 4×4×48 cells per chunk
   (cellWidth 4, cellHeight 8), two sliding 2D slices, `advanceCellX` /
   `selectCellYZ` / `updateForY/X/Z` / `getInterpolatedState`, and the
   block-state filler chain.

Note: **noise caves** (cheese/spaghetti/noodle) live here as density
functions — our current 2-field cave carve is replaced by this layer plus the
procedural carvers in Layer 6.

## Layer 3 — Chunk fill: `NoiseBasedChunkGenerator.doFill` + aquifers + ore veins

- **Fill loop** — exact iteration order (X→Z→Y-descending cells, then
  yInCell↓, xInCell, zInCell), section writes, and `OCEAN_FLOOR_WG` /
  `WORLD_SURFACE_WG` worldgen heightmaps (new heightmap kinds for us).
- **`Aquifer.NoiseBasedAquifer`** (515 ln) — local water/lava bodies below
  the preliminary surface: 16×12×16 grid of aquifer cells with random
  offsets, 3-nearest-cell similarity blending, barrier-noise pressure,
  floodedness/spread/lava noises, per-cell fluid levels; decides
  water/lava/air/solid at every carved-out block. Currently deferred in Vela;
  for parity it is mandatory (it also determines where underground lava
  pockets sit, replacing our flat "lava floor").
- **`OreVeinifier`** (81 ln) — the large copper (y 0..50) and deepslate-iron
  (y −60..−8) veins driven by `veinToggle`/`veinRidged`/`veinGap` + the
  `oreRandom` positional factory (granite/tuff filler, 2% raw-ore blocks).
- **Bedrock is NOT placed here** — it's a `vertical_gradient` surface rule
  (Layer 5); our probabilistic floor goes away.
- **`Beardifier`** — structure terrain adaptation folded into final density
  (24³ Gaussian kernel). Depends on structures (Layer 7); until then the
  no-op marker (contributes 0.0) is *exactly* vanilla behavior wherever no
  adapting structure is nearby — but terrain near villages etc. will differ
  until structures land.
- **`Blender`** (570 ln) — old-chunk blending. **Skippable with zero parity
  cost**: fresh worlds have no blending data, and vanilla's no-blending path
  (`BlendAlpha`=1, `BlendOffset`=0) is what we implement. Document as
  permanently out of scope unless we import pre-1.18 worlds.

## Layer 4 — Biomes: climate space + `MultiNoiseBiomeSource`

Replaces the 16-biome temp/humidity matrix with the real system:

- **`Climate`** (566 ln): 6-D quantized `TargetPoint` (values ×10000 as
  longs), `ParameterPoint` with 7th "offset" dimension, and the **`RTree`**
  nearest-neighbor index (branching factor 6, best-split build, squared-
  distance search with pruning). The tree build order affects tie-breaking —
  port it, don't substitute a generic kd-tree.
- **`OverworldBiomeBuilder`** (1124 ln): the full parameter table — 5
  temperature × 5 humidity bands, 7 continentalness ranges, 7 erosion ranges,
  weirdness slices (valleys/low/mid/high/peaks), the 5×5 biome matrices
  (middle / middle-variant / plateau / plateau-variant / shattered), ocean
  2×5 table, and underground biomes (dripstone, lush, deep dark, sulfur
  caves) → ~1000+ parameter points covering **48 overworld biomes** (we
  currently model 16 of 67 total).
- **`MultiNoiseBiomeSource.getNoiseBiome(quartX, quartY, quartZ)`** sampling
  the router's temperature/vegetation/continents/erosion/depth/ridges at
  quart resolution, and **`fillBiomesFromNoise`** writing the per-section
  4×4×4 biome containers — Vela's biome grid gains vertical variation
  (cave biomes) for the first time.
- Biome IDs already sync correctly (65-entry registry passthrough), so the
  client side is ready.

## Layer 5 — Surface: `SurfaceRules` interpreter + vanilla rule tree

- **Rule engine** (`SurfaceRules.java`, 919 ln): 11 condition sources
  (biome, noise-threshold, vertical-gradient, y-check, water, temperature,
  steep, hole, above-preliminary-surface, stone-depth, not) and 4 rule
  sources (block, sequence, test, bandlands), evaluated lazily per column
  with memoized per-position state.
- **`SurfaceSystem`** (336 ln): per-column application — noise-driven
  surface depth (`Noises.SURFACE`, secondary depth), stone-depth-above/below
  tracking, water tracking, plus the special generators: **eroded-badlands
  terracotta pillars**, **frozen-ocean icebergs**, and the 192-entry
  **clay-band** array.
- **`SurfaceRuleData.overworld()`** (400 ln): the actual vanilla rule tree —
  bedrock floor/roof vertical gradients, badlands bands, swamp puddles,
  frozen peaks / snowy slopes / grove powder-snow branches, windswept
  gravel/stone, ocean sand/sandstone, taiga podzol, mycelium, deepslate
  vertical gradient, default grass/dirt.

Replaces all of `surface.rs` (including bedrock and deepslate, which today
are ad-hoc hashes).

## Layer 6 — Carvers (procedural caves & canyons)

`carver/` (11 classes): `CaveWorldCarver` (probabilistic starts, 0–15 caves,
recursive branching tunnels with sine-envelope radii, occasional rooms) and
`CanyonWorldCarver` (single ravine with per-height width smoothing).

Parity-critical mechanics:
- Per-source-chunk seeding: for each chunk within the carver range around the
  target, `setLargeFeatureSeed(seed + carverIndex, srcX, srcZ)` on the
  **legacy** LCG (`WorldgenRandom`), so carvers cross chunk borders
  deterministically.
- **`CarvingMask`** bitset per chunk; ellipsoid carve with `shouldSkip`
  envelope; carved blocks resolved through the **aquifer**
  (`getCarveState`) — carvers and aquifers are coupled.
- Runs as its own pipeline step with an 8-chunk read radius, writing only the
  center chunk.

## Layer 7 — Features & decoration

The largest volume of code, mostly mechanical:

- **Staged decoration** (`ChunkGenerator.applyBiomeDecoration`): 11
  `GenerationStep.Decoration` steps in fixed order (raw_generation, lakes,
  local_modifications, underground_structures, surface_structures,
  strongholds, underground_ores, underground_decoration, fluid_springs,
  vegetal_decoration, top_layer_modification); features of all biomes present
  in the chunk's neighborhood are unioned into **one global indexed list**
  (vanilla's `FeatureSorter` topological order — the per-feature index feeds
  the seed, so ordering is parity-critical); per feature:
  `setFeatureSeed(decorationSeed, index, step)`.
- **`WorldGenRegion` with write radius 1** — features may write into
  neighboring chunks. This kills our "features stay in their origin chunk"
  simplification and is the main reason the staged pipeline (Layer 8) is
  required.
- **Feature implementations**: 77 `Feature` types; for the overworld the
  heavy hitters are the **tree system** (9+ trunk placers, 11 foliage
  placers, 8 decorators), **`OreFeature`** (line-segment ellipsoids with
  overlap culling + `discardChanceOnAirExposure` — replaces our random-walk
  blobs), disks, lakes, springs, geodes, dripstone/speleothems, vegetation
  patches, kelp/seagrass/coral, `FREEZE_TOP_LAYER` (replaces our snow/ice
  hack), monster rooms, fossils, desert wells…
- **Placement modifiers** (~17–23 types): count, rarity, in_square,
  height_range (uniform/trapezoid), heightmap, biome filter, block-predicate
  filter, environment scan, noise-based counts, surface-water-depth, etc.,
  chained as position streams in `PlacedFeature`.
- **Configuration data**: ~543 configured/placed feature registrations
  (counts, y-distributions, block providers per biome) — data extraction, not
  logic.
- Biome → feature lists per step come from each biome's `BiomeGenerationSettings`
  (datapack JSON).

## Layer 8 — Chunk generation pipeline (architectural change)

Vanilla generates through **13 chunk statuses** (empty → structure_starts →
structure_references → biomes → noise → surface → carvers → features →
initialize_light → light → spawn → full) with per-status dependency radii
(structures 8, carvers 1, features 1 with **write radius 1**).

Vela today bakes a chunk in one pure function with no neighbor access. For
parity we need:

- **Proto-chunk model**: chunks that exist at intermediate statuses, with
  status persisted in chunk NBT (`Status` field) so partial generation
  survives restarts.
- **A scheduler** that generates dependencies first (the "pyramid": asking
  for a FULL chunk forces neighbors to at least FEATURES−1, etc.).
- **`WorldGenRegion`**: a view over a square of proto-chunks that routes
  reads/writes to the right chunk during carving/decoration.
- This also naturally fixes the current cross-chunk light-bleed gap noted in
  the roadmap.

This is the biggest *structural* change to Vela (touches `chunk_data.rs`,
storage, and the sim's chunk-send path), independent of any single algorithm.

## Layer 9 — Structures (largest scope; has an asset problem)

~129 Java files: placement (`RandomSpreadStructurePlacement` with
salt/spacing/separation + `setLargeFeatureWithSalt`; `ConcentricRings` for
strongholds), `StructureStart`/pieces, the **jigsaw** system (pools, aliases,
junction expansion — villages, outposts, ancient cities, trail ruins, trial
chambers), 16 bespoke piece generators (mineshaft, monument, mansion, …), and
the **template system** (34 files: NBT `.nbt` templates + 16 processors).

Two special notes:
- **Templates are Mojang assets** shipped in the vanilla datapack, *not*
  derivable from code. Clean-room policy means we cannot commit them; the
  practical path is loading them at runtime from a user-supplied vanilla
  datapack / extracted `server.jar` data, the same way we treat other data.
- **Terrain parity near structures depends on structures**: the Beardifier
  contribution (Layer 3) is nonzero wherever a `TerrainAdjustment != NONE`
  structure start is within 12 blocks. Until structures exist, terrain
  everywhere else is still exact.

## Data extraction (cross-cutting)

A large fraction of "the algorithm" is actually *data*. Rather than
transcribing decompiled Java constants (provenance risk + typo risk), dump the
**vanilla built-in datapack** via Mojang's official data generator
(`java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --server --reports`)
and generate Rust tables from the JSON:

- `worldgen/noise/*` — the ~48 noise parameter sets
- `worldgen/noise_settings/overworld.json` — the full router graph, spline
  knots, surface rule tree, default block/fluid, sea level (this JSON *is*
  `NoiseRouterData` + `SurfaceRuleData` serialized — implementing the JSON
  codecs gives us the graph without transcribing code)
- `worldgen/biome/*` — biome generation settings (feature lists per step,
  carvers per biome)
- `worldgen/multi_noise_biome_source_parameter_list/overworld.json` — the
  full ~1000-point climate table (avoids porting `OverworldBiomeBuilder`)
- `worldgen/configured_feature/*`, `placed_feature/*`,
  `configured_carver/*`, `structure/*`, `structure_set/*`,
  `template_pool/*`, `processor_list/*`

**Recommendation:** build the engine data-driven (parse these JSONs at boot or
via build-time codegen). That collapses Layers 2/4/5/7's *data* into one
extraction pipeline and matches the roadmap's existing "data generation
pipeline" `XL` item.

## Verification strategy

Parity is testable at every layer — build the harness early:

1. **RNG golden tests**: Xoroshiro sequences, `fromHashOf`/`at` outputs vs
   values captured from the reference JVM.
2. **Noise golden tests**: `NormalNoise.getValue(x,y,z)` for each named noise
   at fixed seeds/coords.
3. **Router golden tests**: each of the 15 router outputs sampled at a grid of
   positions (dump from a small Java harness against the real jar).
4. **Climate/biome tests**: `biome_at(quartX, quartY, quartZ)` over a region.
5. **End-to-end region diff**: run the real `server.jar` with seed S, force a
   radius of chunks, then diff Vela's `.mca` output block-for-block (and
   biome-for-biome) against it. This is the ultimate acceptance test and can
   gate each milestone (noise-only diff first with surface/carvers/features
   disabled via a debug flag, then progressively enable layers).

## Suggested milestones

| # | Milestone | Size | Delivers |
|---|---|---|---|
| P0 | Xoroshiro + positional forking + `RandomState` | `M` | seeding foundation, golden tests |
| P1 | Noise primitives (Improved/Perlin/Normal/Blended) + noise-param extraction | `M` | per-noise golden tests |
| P2 | Density-function engine + cubic splines + overworld router (JSON-codec-driven) + `NoiseChunk` + `doFill` | `XL` | vanilla terrain *shape* (stone/air/water), noise caves included |
| P3 | Aquifers + ore veins | `L` | correct underground fluids + copper/iron veins |
| P4 | Climate sampler + RTree + parameter list + `fillBiomesFromNoise` | `L` | exact biomes (48 overworld incl. cave biomes) |
| P5 | SurfaceRules engine + vanilla rule tree | `L` | exact surface blocks, bedrock, deepslate |
| P6 | Staged chunk pipeline (proto-chunks, statuses, `WorldGenRegion`) | `L` | architecture for carvers/features/structures |
| P7 | Carvers + carving masks (aquifer-coupled) | `M` | ravines + carver caves |
| P8 | Feature engine (placement modifiers, decoration order/seeding) + feature impls + data | `XL` | trees/ores/vegetation, cross-chunk, exact |
| P9 | Structures (placement → jigsaw → pieces → Beardifier wiring) | `XL` | villages etc.; last gap in terrain parity |

P2 is the watershed: after it, a seed's mountains and coastlines match
vanilla maps (verifiable against any seed-map tool). P0–P5 can land without
touching the chunk pipeline (all write-radius-0); P6 must precede P7–P9.

Out of scope for parity of fresh worlds: `Blender` (pre-1.18 chunk
blending), Nether/End dimensions (same engine, different router + biome
sources — cheap to add once the engine exists).
