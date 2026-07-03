//! Ad-hoc stage profiler for the parity worldgen pipeline.
//! Run: cargo run --release --example profile_gen
use std::time::Instant;

use vela::world::gen::pipeline::{self, ChunkPipeline, ChunkStatus};

fn main() {
    let seed = 0x5EED_C0DE_i64;

    // ---- Stage attribution on a private pipeline instance ------------------
    let mut p = ChunkPipeline::new_overworld(seed);

    let mut area = |r: i32, status: ChunkStatus| -> (f64, usize) {
        let start = Instant::now();
        let mut n = 0usize;
        for dz in -r..=r {
            for dx in -r..=r {
                p.advance(dx, dz, status);
                n += 1;
            }
        }
        (start.elapsed().as_secs_f64(), n)
    };

    // Level-by-level so each bucket contains only its own stage's work.
    let (t_biomes, n_biomes) = area(4, ChunkStatus::Biomes);
    let (t_noise, n_noise) = area(3, ChunkStatus::Noise);
    let (t_surface, n_surface) = area(3, ChunkStatus::Surface);
    let (t_carvers, n_carvers) = area(2, ChunkStatus::Carvers);

    println!("biomes : {:8.2} ms/chunk  ({} chunks)", t_biomes * 1e3 / n_biomes as f64, n_biomes);
    println!("noise  : {:8.2} ms/chunk  ({} chunks)", t_noise * 1e3 / n_noise as f64, n_noise);
    println!("surface: {:8.2} ms/chunk  ({} chunks)", t_surface * 1e3 / n_surface as f64, n_surface);
    println!("carvers: {:8.2} ms/chunk  ({} chunks)", t_carvers * 1e3 / n_carvers as f64, n_carvers);

    // ---- Steady-state marginal cost of the full live path ------------------
    // generate_full uses the thread-local pipeline; walk a fresh strip far from
    // the area above. First call warms the 5x5, the rest are the marginal cost
    // a strip crossing pays per chunk.
    let mut times = Vec::new();
    for cx in 0..14 {
        let start = Instant::now();
        std::hint::black_box(pipeline::generate_full(cx, 1000));
        times.push(start.elapsed().as_secs_f64());
    }
    println!("\ngenerate_full strip (cz=1000):");
    for (cx, t) in times.iter().enumerate() {
        println!("  cx={cx:2}  {:8.2} ms", t * 1e3);
    }
    let steady = &times[4..];
    let avg = steady.iter().sum::<f64>() / steady.len() as f64;
    println!("steady-state marginal: {:.2} ms/chunk", avg * 1e3);

    // ---- Full client-ready product: generate + light + wire-encode ---------
    // Same thread-local pipeline; fresh strip. The delta vs generate_full's
    // marginal is what lighting + encoding + store bookkeeping add.
    vela::world::set_seed(seed);
    let mut times = Vec::new();
    for cx in 0..14 {
        let start = Instant::now();
        std::hint::black_box(vela::world::chunk_columns(cx, 2000));
        times.push(start.elapsed().as_secs_f64());
        vela::world::evict_chunk(cx, 2000, 0);
    }
    println!("\nchunk_columns strip (cz=2000):");
    for (cx, t) in times.iter().enumerate() {
        println!("  cx={cx:2}  {:8.2} ms", t * 1e3);
    }
    let steady = &times[4..];
    let avg = steady.iter().sum::<f64>() / steady.len() as f64;
    println!("steady-state marginal (full wire): {:.2} ms/chunk", avg * 1e3);
}
