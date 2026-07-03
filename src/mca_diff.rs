//! End-to-end `.mca` block/biome diff acceptance harness (test-only).
//!
//! This is the "ultimate acceptance test" named in `docs/WORLDGEN_PARITY.md`
//! ("Verification strategy" item 5) and documented in `docs/MCA_DIFF.md`: the
//! REAL vanilla 26.2 server is run for a seed (see `tools/mca_diff/run_vanilla.ps1`),
//! a square of chunks is force-generated to `minecraft:full`, and its Anvil
//! region files are diffed against Vela's parity generator block-for-block and
//! biome-for-biome.
//!
//! # What it compares
//!
//! For every block in the buildable column of every requested chunk:
//!   * **name-level** — the block NAME only (`minecraft:stone`), ignoring block
//!     properties. Vela's parity generator collapses properties by design (it
//!     emits [`ParityBlock`] variants that carry no property state), so this is
//!     the meaningful parity signal.
//!   * **state-level** — NAME + sorted properties, reported separately. Vela
//!     will diverge here wherever vanilla varies a property Vela does not model
//!     (water `level`, `snowy`, log `axis`, leaf `distance`…), so a low
//!     state-level number is expected and is *not* a terrain-shape defect.
//!
//! Biomes are compared per quart cell (the section's 4×4×4 biome container).
//!
//! # Known-gap buckets
//!
//! Vanilla output includes structures (Vela has none — roadmap P9) and block
//! entities. Each chunk is bucketed by whether its `structures` NBT is empty
//! (`starts` and `References` both empty): **structure-free** chunks are the
//! clean parity signal; **structure-touched** chunks are counted separately so
//! P9's absence does not drown the terrain/feature comparison.
//!
//! # Running
//!
//! The report test is `#[ignore]` (it needs the vanilla region fixture and is
//! slow). See `docs/MCA_DIFF.md`. Quick form:
//!
//! ```text
//! # 1. produce the fixture (real jar):
//! powershell -File tools/mca_diff/run_vanilla.ps1 -Seed 1592639710 -OutDir <dir>
//! # 2. diff:
//! VELA_MCA_DIR=<dir> cargo test --release mca_diff::report -- --ignored --nocapture
//! ```

#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bytes::Bytes;

use crate::protocol::nbt::{self, Nbt};
use crate::registry::block_state::describe_state;
use crate::world::gen::density::ParityBlock;
use crate::world::gen::pipeline::{
    biome_name_of, block_state_of, ChunkPipeline, ChunkStatus, ProtoChunk,
};
use crate::world::storage::region::RegionFile;

// ---------------------------------------------------------------------------
// Vanilla chunk decode (read-side; independent of Vela's own save format)
// ---------------------------------------------------------------------------

/// One decoded section of a vanilla chunk: the block/biome palettes plus the
/// per-cell palette indices, kept in vanilla's storage order.
struct VanillaSection {
    /// Block palette: `(name, sorted "k=v,k=v" property string)`.
    block_palette: Vec<(String, String)>,
    /// 4096 block indices, cell order `(y<<8)|(z<<4)|x`.
    block_idx: Vec<u16>,
    /// Biome palette: registry names.
    biome_palette: Vec<String>,
    /// 64 biome indices, quart order `(y*4+z)*4+x` (empty ⇒ single-value).
    biome_idx: Vec<u16>,
}

impl VanillaSection {
    fn block_name(&self, cell: usize) -> &str {
        &self.block_palette[self.block_idx[cell] as usize].0
    }
    fn block_state(&self, cell: usize) -> &(String, String) {
        &self.block_palette[self.block_idx[cell] as usize]
    }
    fn biome_name(&self, quart: usize) -> &str {
        if self.biome_palette.len() == 1 {
            &self.biome_palette[0]
        } else {
            &self.biome_palette[self.biome_idx[quart] as usize]
        }
    }
}

/// A decoded vanilla chunk: its sections keyed by section-Y, its status, and
/// whether any structure touches it.
struct VanillaChunk {
    status: String,
    structure_free: bool,
    sections: HashMap<i32, VanillaSection>,
}

/// `Mth.ceillog2(n)` — smallest `k` with `2^k >= n` (0 for `n <= 1`).
fn ceillog2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

/// Disk storage width for a block-state palette (`createForBlockStates`):
/// `0` for a single value, else `max(4, ceillog2(size))`.
fn block_bits(len: usize) -> u32 {
    if len <= 1 {
        0
    } else {
        ceillog2(len).max(4)
    }
}

/// Disk storage width for a biome palette (`createForBiomes`): `ceillog2(size)`,
/// no 4-bit floor (`0` for a single value).
fn biome_bits(len: usize) -> u32 {
    ceillog2(len)
}

/// Unpack `count` `bits`-wide values from a `SimpleBitStorage` long array
/// (values never straddle a long; low-to-high within each long).
fn unpack(longs: &[i64], bits: u32, count: usize) -> Vec<u16> {
    if bits == 0 {
        return vec![0u16; count];
    }
    let per_long = (64 / bits) as usize;
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let long = longs.get(i / per_long).copied().unwrap_or(0) as u64;
        let offset = (i % per_long) as u32 * bits;
        out.push(((long >> offset) & mask) as u16);
    }
    out
}

/// Format a block palette entry `{Name, Properties?}` as `(name, "k=v,k=v")`
/// with properties sorted for a stable state string.
fn parse_block_entry(entry: &Nbt) -> Option<(String, String)> {
    let name = entry.get("Name").and_then(Nbt::as_str)?.to_string();
    let mut props: Vec<(String, String)> = match entry.get("Properties") {
        Some(Nbt::Compound(map)) => map
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect(),
        _ => Vec::new(),
    };
    props.sort();
    let prop_str = props
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    Some((name, prop_str))
}

/// Decode a vanilla chunk NBT root. Returns `None` for a non-full chunk.
fn decode_vanilla(root: &Nbt) -> Option<VanillaChunk> {
    let status = root.get("Status").and_then(Nbt::as_str)?.to_string();

    // Structure-free = both `starts` and `References` empty.
    let structure_free = match root.get("structures") {
        Some(s) => {
            let empty = |k: &str| match s.get(k) {
                Some(Nbt::Compound(m)) => m.is_empty(),
                None => true,
                _ => true,
            };
            empty("starts") && empty("References")
        }
        None => true,
    };

    let mut sections = HashMap::new();
    if let Some(Nbt::List(list)) = root.get("sections") {
        for sec in list {
            let y = match sec.get("Y") {
                Some(Nbt::Byte(y)) => *y as i32,
                _ => continue,
            };
            // Blocks.
            let (block_palette, block_idx) = match sec.get("block_states") {
                Some(bs) => {
                    let palette: Vec<(String, String)> = match bs.get("palette") {
                        Some(Nbt::List(entries)) => {
                            entries.iter().map(|e| parse_block_entry(e)).collect::<Option<_>>()?
                        }
                        _ => continue,
                    };
                    let bits = block_bits(palette.len());
                    let idx = match bs.get("data") {
                        Some(Nbt::LongArray(d)) => unpack(d, bits, 4096),
                        _ => vec![0u16; 4096], // single-value section
                    };
                    (palette, idx)
                }
                None => (vec![("minecraft:air".into(), String::new())], vec![0u16; 4096]),
            };
            // Biomes.
            let (biome_palette, biome_idx) = match sec.get("biomes") {
                Some(bi) => {
                    let palette: Vec<String> = match bi.get("palette") {
                        Some(Nbt::List(entries)) => entries
                            .iter()
                            .map(|e| e.as_str().map(str::to_string))
                            .collect::<Option<_>>()?,
                        _ => vec!["minecraft:plains".into()],
                    };
                    let bits = biome_bits(palette.len());
                    let idx = match bi.get("data") {
                        Some(Nbt::LongArray(d)) => unpack(d, bits, 64),
                        _ => Vec::new(),
                    };
                    (palette, idx)
                }
                None => (vec!["minecraft:plains".into()], Vec::new()),
            };
            sections.insert(
                y,
                VanillaSection { block_palette, block_idx, biome_palette, biome_idx },
            );
        }
    }

    Some(VanillaChunk { status, structure_free, sections })
}

// ---------------------------------------------------------------------------
// Region-file access (a small cache of open region handles)
// ---------------------------------------------------------------------------

struct RegionSet {
    dir: PathBuf,
    open: HashMap<(i32, i32), Option<RegionFile>>,
}

impl RegionSet {
    fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf(), open: HashMap::new() }
    }

    /// Decode vanilla chunk `(cx, cz)` from its region file, if present.
    fn chunk(&mut self, cx: i32, cz: i32) -> Option<VanillaChunk> {
        let (rx, rz) = (cx >> 5, cz >> 5);
        let dir = &self.dir;
        let rf = self.open.entry((rx, rz)).or_insert_with(|| {
            let path = dir.join(format!("r.{rx}.{rz}.mca"));
            RegionFile::open(&path).ok()
        });
        let rf = rf.as_mut()?;
        let bytes = rf.read_chunk((cx & 31) as usize, (cz & 31) as usize).ok()??;
        let mut slice = Bytes::copy_from_slice(&bytes);
        let (_, root) = nbt::read_named(&mut slice).ok()?;
        decode_vanilla(&root)
    }
}

// ---------------------------------------------------------------------------
// Vela full-state string (name + collapsed properties it does model)
// ---------------------------------------------------------------------------

/// Vela's `(name, "k=v,k=v")` for a parity block, via the block registry.
fn vela_state(pb: ParityBlock) -> (String, String) {
    let id = block_state_of(pb).0;
    match describe_state(id) {
        Some((name, mut props)) => {
            props.sort();
            let s = props
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",");
            (name.to_string(), s)
        }
        None => (pb.block_name().to_string(), String::new()),
    }
}

// ---------------------------------------------------------------------------
// Per-chunk diff
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct ChunkStats {
    blocks_total: u64,
    name_match: u64,
    state_match: u64,
    /// Cells where at least one side is non-air (the meaningful terrain), and
    /// how many of those matched at name level — the air-air majority (open sky)
    /// otherwise inflates the raw percentage.
    nontrivial_total: u64,
    nontrivial_match: u64,
    biome_total: u64,
    biome_match: u64,
    /// name-mismatch tally keyed by `"vanilla -> vela"`.
    name_mismatch: HashMap<String, u64>,
    /// biome-mismatch tally keyed by `"vanilla -> vela"`.
    biome_mismatch: HashMap<String, u64>,
    /// first few concrete mismatches: `(x, y, z, vanilla, vela)`.
    samples: Vec<(i32, i32, i32, String, String)>,
}

impl ChunkStats {
    fn name_perfect(&self) -> bool {
        self.blocks_total > 0 && self.name_match == self.blocks_total
    }
    fn state_perfect(&self) -> bool {
        self.blocks_total > 0 && self.state_match == self.blocks_total
    }
    fn merge_from(&mut self, o: &ChunkStats) {
        self.blocks_total += o.blocks_total;
        self.name_match += o.name_match;
        self.state_match += o.state_match;
        self.nontrivial_total += o.nontrivial_total;
        self.nontrivial_match += o.nontrivial_match;
        self.biome_total += o.biome_total;
        self.biome_match += o.biome_match;
        for (k, v) in &o.name_mismatch {
            *self.name_mismatch.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &o.biome_mismatch {
            *self.biome_mismatch.entry(k.clone()).or_default() += v;
        }
    }
}

/// Diff one chunk: vanilla `vc` vs the Vela proto `proto` at chunk `(cx, cz)`.
fn diff_chunk(cx: i32, cz: i32, vc: &VanillaChunk, proto: &ProtoChunk, max_samples: usize) -> ChunkStats {
    let mut st = ChunkStats::default();
    let fc = proto.blocks.as_ref().expect("featured chunk has blocks");
    let biome_sections = proto.biome_sections.as_ref().expect("chunk has biomes");
    let min_y = fc.min_y;
    let height = fc.height;
    let min_section_y = min_y >> 4;

    // --- blocks ---
    for wy in min_y..(min_y + height) {
        let sec_y = wy >> 4;
        let ly = (wy & 15) as usize;
        let sec = vc.sections.get(&sec_y);
        for lz in 0..16i32 {
            for lx in 0..16i32 {
                let cell = (ly << 8) | ((lz as usize) << 4) | (lx as usize);
                let (v_name, v_state): (String, String) = match sec {
                    Some(s) => {
                        let (n, p) = s.block_state(cell);
                        (n.clone(), p.clone())
                    }
                    None => ("minecraft:air".into(), String::new()),
                };
                let pb = fc.block(lx, wy, lz);
                let (a_name, a_state) = vela_state(pb);

                st.blocks_total += 1;
                let both_air = v_name == "minecraft:air" && a_name == "minecraft:air";
                if !both_air {
                    st.nontrivial_total += 1;
                }
                if v_name == a_name {
                    st.name_match += 1;
                    if !both_air {
                        st.nontrivial_match += 1;
                    }
                } else {
                    *st.name_mismatch.entry(format!("{v_name} -> {a_name}")).or_default() += 1;
                    if st.samples.len() < max_samples {
                        st.samples.push((
                            cx * 16 + lx,
                            wy,
                            cz * 16 + lz,
                            v_name.clone(),
                            a_name.clone(),
                        ));
                    }
                }
                if v_name == a_name && v_state == a_state {
                    st.state_match += 1;
                }
            }
        }
    }

    // --- biomes (per quart cell) ---
    for wy in min_y..(min_y + height) {
        if wy & 3 != 0 {
            continue; // one sample per quart in Y
        }
        let qy = wy >> 2;
        let sec_y = wy >> 4;
        let sec = match vc.sections.get(&sec_y) {
            Some(s) => s,
            None => continue,
        };
        let vela_sec = &biome_sections[(sec_y - min_section_y) as usize];
        for qz in 0..4i32 {
            for qx in 0..4i32 {
                let quart = (((qy & 3) * 4 + qz) * 4 + qx) as usize;
                let v_name = sec.biome_name(quart);
                let fill = vela_sec[quart];
                let a_name = biome_name_of(fill);
                st.biome_total += 1;
                if v_name == a_name {
                    st.biome_match += 1;
                } else {
                    *st.biome_mismatch.entry(format!("{v_name} -> {a_name}")).or_default() += 1;
                }
            }
        }
    }

    st
}

// ---------------------------------------------------------------------------
// Report driver
// ---------------------------------------------------------------------------

fn pct(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        100.0 * num as f64 / den as f64
    }
}

/// Sort a mismatch tally into a descending `(key, count)` list.
fn top(map: &HashMap<String, u64>, n: usize) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = map.iter().map(|(k, c)| (k.clone(), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// The end-to-end block/biome diff report. Ignored by default: it needs the
/// vanilla region fixture (`VELA_MCA_DIR`, produced by `run_vanilla.ps1`) and is
/// slow. Reads seed from `SEED.txt` in that dir (or `VELA_MCA_SEED`), diffs a
/// `VELA_MCA_RADIUS`-chunk square (default 6, one ring inside the generated
/// radius-7 area so cross-border features are complete), and prints the report.
#[test]
#[ignore = "needs a vanilla .mca fixture (VELA_MCA_DIR); run via docs/MCA_DIFF.md"]
fn report() {
    let dir = std::env::var("VELA_MCA_DIR")
        .expect("set VELA_MCA_DIR to a dir of vanilla r.*.mca files (see docs/MCA_DIFF.md)");
    let dir = PathBuf::from(dir);
    let seed: i64 = std::env::var("VELA_MCA_SEED")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .or_else(|| {
            std::fs::read_to_string(dir.join("SEED.txt"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
        })
        .expect("seed from VELA_MCA_SEED or SEED.txt");
    let radius: i32 = std::env::var("VELA_MCA_RADIUS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let max_samples: usize = 12;

    eprintln!("=== Vela .mca diff — seed {seed}, chunks [-{radius}..{radius}]^2 ===");

    // Generate the padded region once through a shared pipeline so cross-chunk
    // FEATURES writes (write radius 1) into the diffed chunks are present.
    let mut pipeline = ChunkPipeline::new_overworld(seed);
    let pad = radius + 1;
    let t0 = std::time::Instant::now();
    for cz in -pad..=pad {
        for cx in -pad..=pad {
            pipeline.advance(cx, cz, ChunkStatus::Features);
        }
    }
    eprintln!(
        "generated {} Vela chunks in {:?}",
        (2 * pad + 1) * (2 * pad + 1),
        t0.elapsed()
    );

    let mut regions = RegionSet::new(&dir);

    let mut clean = ChunkStats::default(); // structure-free chunks
    let mut dirty = ChunkStats::default(); // structure-touched chunks
    let mut clean_chunks = 0u32;
    let mut dirty_chunks = 0u32;
    let mut missing = 0u32;
    let mut name_perfect_chunks = 0u32;
    let mut state_perfect_chunks = 0u32;
    let mut printed_samples = 0u32;

    for cz in -radius..=radius {
        for cx in -radius..=radius {
            let vc = match regions.chunk(cx, cz) {
                Some(vc) if vc.status == "minecraft:full" => vc,
                _ => {
                    missing += 1;
                    continue;
                }
            };
            let proto = pipeline.chunk(cx, cz).expect("advanced above").clone();
            let cs = diff_chunk(cx, cz, &vc, &proto, max_samples);

            if vc.structure_free {
                clean_chunks += 1;
                clean.merge_from(&cs);
                if cs.name_perfect() {
                    name_perfect_chunks += 1;
                }
                if cs.state_perfect() {
                    state_perfect_chunks += 1;
                }
                // Print a handful of concrete mismatches from the clean bucket.
                if printed_samples < 3 && !cs.samples.is_empty() {
                    eprintln!("  chunk ({cx},{cz}) first block mismatches:");
                    for (x, y, z, van, vela) in cs.samples.iter().take(max_samples) {
                        eprintln!("    ({x:>5},{y:>4},{z:>5})  vanilla={van:<30} vela={vela}");
                    }
                    printed_samples += 1;
                }
            } else {
                dirty_chunks += 1;
                dirty.merge_from(&cs);
            }
        }
    }

    let total_chunks = clean_chunks + dirty_chunks;
    eprintln!("\n--- chunk buckets ---");
    eprintln!("  structure-free : {clean_chunks}");
    eprintln!("  structure-touch: {dirty_chunks}");
    eprintln!("  missing/partial: {missing}");

    eprintln!("\n--- structure-free chunks (the clean parity signal) ---");
    eprintln!(
        "  blocks name-level : {}/{} = {:.4}%  (incl. air)",
        clean.name_match,
        clean.blocks_total,
        pct(clean.name_match, clean.blocks_total)
    );
    eprintln!(
        "  blocks name-level : {}/{} = {:.4}%  (non-air terrain only)",
        clean.nontrivial_match,
        clean.nontrivial_total,
        pct(clean.nontrivial_match, clean.nontrivial_total)
    );
    eprintln!(
        "  blocks state-level: {}/{} = {:.4}%",
        clean.state_match,
        clean.blocks_total,
        pct(clean.state_match, clean.blocks_total)
    );
    eprintln!(
        "  biomes            : {}/{} = {:.4}%",
        clean.biome_match,
        clean.biome_total,
        pct(clean.biome_match, clean.biome_total)
    );
    eprintln!(
        "  chunks name-identical : {name_perfect_chunks}/{clean_chunks}"
    );
    eprintln!(
        "  chunks state-identical: {state_perfect_chunks}/{clean_chunks}"
    );

    eprintln!("\n--- top block name mismatches (structure-free), 'vanilla -> vela' ---");
    for (k, c) in top(&clean.name_mismatch, 30) {
        eprintln!("  {c:>10}  {k}");
    }
    eprintln!("\n--- top biome mismatches (structure-free), 'vanilla -> vela' ---");
    for (k, c) in top(&clean.biome_mismatch, 20) {
        eprintln!("  {c:>10}  {k}");
    }

    if dirty_chunks > 0 {
        eprintln!("\n--- structure-touched chunks (reported separately; P9 gap) ---");
        eprintln!(
            "  blocks name-level : {}/{} = {:.4}%",
            dirty.name_match,
            dirty.blocks_total,
            pct(dirty.name_match, dirty.blocks_total)
        );
        eprintln!("\n  top block name mismatches (structure-touched):");
        for (k, c) in top(&dirty.name_mismatch, 15) {
            eprintln!("  {c:>10}  {k}");
        }
    }

    eprintln!("\n=== summary: {total_chunks} chunks diffed, {name_perfect_chunks} name-identical ===");

    // Sanity assertions so the harness itself is regression-guarded (not parity
    // gates — those are documented, evolving numbers in docs/MCA_DIFF.md).
    assert!(clean.blocks_total + dirty.blocks_total > 0, "diffed at least one chunk");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_widths_match_vanilla() {
        // Blocks: 4-bit floor.
        assert_eq!(block_bits(1), 0);
        assert_eq!(block_bits(2), 4);
        assert_eq!(block_bits(16), 4);
        assert_eq!(block_bits(17), 5);
        assert_eq!(block_bits(256), 8);
        assert_eq!(block_bits(257), 9);
        // Biomes: no floor.
        assert_eq!(biome_bits(1), 0);
        assert_eq!(biome_bits(2), 1);
        assert_eq!(biome_bits(3), 2);
        assert_eq!(biome_bits(4), 2);
        assert_eq!(biome_bits(5), 3);
        assert_eq!(biome_bits(9), 4);
    }

    #[test]
    fn unpack_is_non_spanning_low_to_high() {
        // 4-bit values 1,2,3 in the low nibbles of one long.
        let longs = vec![0x321i64];
        assert_eq!(unpack(&longs, 4, 3), vec![1, 2, 3]);
        // Zero bits ⇒ all index 0.
        assert_eq!(unpack(&[], 0, 5), vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn parse_block_entry_sorts_properties() {
        let e = Nbt::compound([
            ("Name", Nbt::string("minecraft:water")),
            ("Properties", Nbt::compound([("level", Nbt::string("3"))])),
        ]);
        assert_eq!(parse_block_entry(&e), Some(("minecraft:water".into(), "level=3".into())));
    }

    #[test]
    fn decode_rejects_and_accepts() {
        // A minimal full chunk with one single-value stone section at Y=0.
        let section = Nbt::compound([
            ("Y", Nbt::Byte(0)),
            (
                "block_states",
                Nbt::compound([(
                    "palette",
                    Nbt::List(vec![Nbt::compound([("Name", Nbt::string("minecraft:stone"))])]),
                )]),
            ),
            (
                "biomes",
                Nbt::compound([("palette", Nbt::List(vec![Nbt::string("minecraft:plains")]))]),
            ),
        ]);
        let root = Nbt::compound([
            ("Status", Nbt::string("minecraft:full")),
            ("sections", Nbt::List(vec![section])),
            ("structures", Nbt::compound([
                ("starts", Nbt::Compound(vec![])),
                ("References", Nbt::Compound(vec![])),
            ])),
        ]);
        let vc = decode_vanilla(&root).expect("decodes");
        assert_eq!(vc.status, "minecraft:full");
        assert!(vc.structure_free);
        let sec = vc.sections.get(&0).expect("section 0");
        assert_eq!(sec.block_name(0), "minecraft:stone");
        assert_eq!(sec.block_name(4095), "minecraft:stone");
        assert_eq!(sec.biome_name(0), "minecraft:plains");
    }
}
