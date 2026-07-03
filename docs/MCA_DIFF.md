# End-to-end `.mca` block/biome diff harness

The **ultimate worldgen acceptance test** named in
[`WORLDGEN_PARITY.md`](WORLDGEN_PARITY.md) ("Verification strategy" item 5):
run the REAL vanilla 26.2 server for a seed, force a square of chunks to
generate to `minecraft:full`, then diff Vela's parity generator against
vanilla's region files **block-for-block and biome-for-biome**.

Unlike the per-layer golden fixtures (which compare against a JVM transcription
harness), this diffs the *whole pipeline output* against the shipping server —
so it catches any divergence the layer fixtures don't, and it categorizes what's
left to do by the block/feature responsible.

## Pieces

| Piece | Path | What it does |
|---|---|---|
| Vanilla runner | `tools/mca_diff/run_vanilla.ps1` | boots the real jar headless, `forceload`s a chunk square, saves, collects `*.mca` |
| Vela generator + differ | `src/mca_diff.rs` (`#[ignore]` test `mca_diff::report`) | regenerates the same chunks via `ChunkPipeline`, decodes vanilla chunks, diffs |

The differ reuses Vela's Anvil reader (`world::storage::region::RegionFile`) and
NBT parser (`protocol::nbt`), and writes its own **vanilla-chunk decoder** —
vanilla stores per-section `block_states`/`biomes` paletted containers with
`{Name, Properties}` palettes, which differ from Vela's own save format. The
storage-width formulas are transcribed from `PalettedContainer.Strategy`
(blocks: `max(4, ceillog2(size))`; biomes: `ceillog2(size)`; single value ⇒ 0).

## Usage

### 1. Produce the vanilla fixture (needs a JVM + the real jar)

```powershell
powershell -File tools/mca_diff/run_vanilla.ps1 `
  -Seed 1592639710 `
  -OutDir C:\tmp\mca\1592639710 `
  -Jar   C:\Users\kiezi\mc-decompile\server.jar `
  -Radius 7
```

This creates a throwaway server dir (`eula=true`, `online-mode=false`,
`level-seed=<Seed>`, `server-port=0`), boots it, waits for `Done`, then drives
the console over stdin: `forceload add` the block square covering chunks
`[-Radius..Radius]²`, `save-all flush`, `stop`. It copies the overworld
`region/*.mca` plus a `SEED.txt` to `-OutDir`.

`forceload` caps at 256 chunks/command, so keep `Radius ≤ 7` (225 chunks).
**Bonus coverage:** vanilla's boot-time spawn-area preparation generates full
chunks well beyond the forced square (out to ≈ radius 14–18 for this seed), so
the fixture usually contains ~1600 full chunks — the differ can diff a much
larger radius than was forced (see `VELA_MCA_RADIUS` below).

Two environment gotchas the runner handles (documented in-script):
* **Windows PowerShell 5.1 stdin BOM** — the redirected `StandardInput` prepends
  a UTF-8 BOM to the *first* byte written, which the server reads as a garbage
  command. The runner sends one throwaway blank line first to absorb it, then
  writes raw ASCII bytes.
* **MC 26.2 dimension layout** — the overworld lives at
  `world/dimensions/minecraft/overworld/region/`, not the legacy `world/region/`.

If no JVM is available, the runner aborts early with a clear message; you can
still exercise the Vela-side + differ against any pre-existing `*.mca` fixture.

### 2. Diff Vela against it

```bash
VELA_MCA_DIR=C:/tmp/mca/1592639710 \
  cargo test --release --bin vela mca_diff::report -- --ignored --nocapture
```

Env knobs:
* `VELA_MCA_DIR` (required) — dir of vanilla `r.*.mca` + `SEED.txt`.
* `VELA_MCA_SEED` — override the seed (else read from `SEED.txt`).
* `VELA_MCA_RADIUS` — chunk radius to diff (default 6; one ring inside the
  forced radius-7 area so cross-chunk FEATURES writes are complete). Raise it to
  cover the spawn-prep chunks (e.g. `12` gives 625 core chunks and catches the
  first structures).

The Vela side builds one `ChunkPipeline::new_overworld(seed)` and advances every
chunk in the padded square to `FEATURES`, so cross-border feature writes
(write-radius 1) land in the diffed chunks the same way vanilla accumulates
them. Each chunk is bucketed **structure-free** vs **structure-touched** by
whether its `structures` NBT (`starts`/`References`) is empty, and the two
buckets are reported separately so the P9 structures gap doesn't drown the
signal.

## Results — seed `1592639710`

Fixture: `Radius 7` forced + spawn-prep; diffed at `VELA_MCA_RADIUS=12`
(361 full chunks diffed, 264 beyond the full-generated area skipped).

| Bucket | Chunks | Blocks name-level (incl. air) | Blocks name-level (non-air) | Biomes |
|---|---|---|---|---|
| **structure-free** | **329** | **96.36 %** | **89.76 %** | **100.00 %** |
| structure-touched (P9 gap) | 32 | 94.93 % | — | — |

- **Biomes are 100.00 % identical** (505 344 quart cells) — the P4/P5 climate +
  biome-zoom stack matches vanilla exactly for this seed.
- State-level (name + properties) ≈ name-level: Vela's parity blocks are almost
  all propertyless or at their default state, so property-collapse costs almost
  nothing here.
- **No chunk is 100 % name-identical** — every chunk carries at least a few
  blocks from a deferred feature — but the terrain *shape* (stone/deepslate/air/
  water baseline) and biomes match.

### Top mismatch causes → follow-up work items

Ranked by block count over the structure-free bucket. Each is a concrete gap the
diff surfaces; none is a terrain-shape defect (the density/surface/carver stack
matches):

1. **Stone-variant & tuff blobs not placed (~680k blocks)** — `tuff → deepslate`
   (357k), `granite → stone|deepslate|andesite|diorite` (~330k). These are the
   `minecraft:ore` stone-blob configured features (`ore_tuff`, `ore_granite`,
   `ore_diorite`, `ore_andesite`, `ore_gravel`) in the `local_modifications`
   decoration step. Vela places *some* diorite/andesite blobs (hence
   `granite → andesite/diorite` overlaps) but not the granite/tuff blobs, so the
   blob feature set is incomplete or the per-biome feature list / target rule for
   these differs. **→ audit the stone-variant ore-blob features end-to-end.**
2. **Trees & vegetation → air (~330k)** — oak/jungle/mangrove `*_leaves`/`*_log`,
   `vine`, `bamboo`, `short_grass`, `fern`, `mangrove_roots`. The deferred P8
   tree + vegetation feature set (`random_patch`, `simple_block`, trunk/foliage
   placers). **→ tree system + vegetation (already on the roadmap).**
3. **Dripstone / geodes / lush caves → deepslate|stone|air (~90k)** —
   `dripstone_block`, `pointed_dripstone`, `smooth_basalt`, `amethyst_block`,
   `calcite`. Deferred P8 cave-decoration features (dripstone clusters, amethyst
   geodes). **→ underground decoration features.**
4. **Surface top-block boundary swaps (~9k)** — `podzol → grass_block`,
   `dirt ↔ grass_block`. Minor: podzol patches in old-growth taiga and grass/dirt
   at feature/decoration edges. **→ revisit once vegetation lands (some are
   feature side-effects), then any residual is a surface-rule edge.**
5. **Feature-ore position drift (small)** — e.g. `coal_ore → stone`. Individual
   ore blobs at slightly different positions; investigate once (1)–(2) shrink the
   noise floor.

**Structure-touched bucket** additionally shows `tuff_bricks → …` (trial
chambers) and `air ↔ deepslate` (mineshaft/structure carving) — these are the
**P9 structures** gap, correctly isolated into their own bucket.

## Reproducing / extending

- Other seeds: pass `-Seed` to the runner; the differ picks the seed up from
  `SEED.txt`.
- Wider forced coverage: raise `-Radius` (≤ 7 per forceload command); to go
  beyond 256 chunks, extend the runner to issue several `forceload add` commands.
- The differ prints the first handful of concrete `(x,y,z) vanilla vs vela`
  mismatches per chunk to make a new divergence easy to locate.
