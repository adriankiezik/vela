//! Aggregate multi-worker serve throughput, mimicking the live
//! prefetch pool — N threads, each with its own thread-local pipeline, chunks
//! sharded by the same 4×4-block splitmix hash as `world::prefetch`.
//! Run: cargo run --release --example profile_parallel
use std::time::Instant;

use vela::world::gen::pipeline;

/// Same routing as `chunk_data::prefetch_shard`.
fn shard(cx: i32, cz: i32, workers: usize) -> usize {
    let bx = ((cx >> 2) as i64) as u64;
    let bz = ((cz >> 2) as i64) as u64;
    let mut h = bx
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(bz.wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    (h % workers as u64) as usize
}

fn main() {
    vela::world::set_seed(0x5EED_C0DE);
    let workers: usize = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2))
        .unwrap_or(2)
        .max(2);

    // The workload: a join-sized square region of serves — what a player landing
    // in fresh terrain at view distance 16 demands (33×33 ≈ 1089 columns).
    let r = 16;
    let (ox, oz) = (100_000, 100_000); // far from anything another run touched
    let mut buckets: Vec<Vec<(i32, i32)>> = vec![Vec::new(); workers];
    for cz in -r..=r {
        for cx in -r..=r {
            let (cx, cz) = (ox + cx, oz + cz);
            buckets[shard(cx, cz, workers)].push((cx, cz));
        }
    }
    let total: usize = buckets.iter().map(Vec::len).sum();
    println!("{total} serves across {workers} workers");

    let start = Instant::now();
    let handles: Vec<_> = buckets
        .into_iter()
        .map(|coords| {
            std::thread::spawn(move || {
                for (cx, cz) in coords {
                    std::hint::black_box(pipeline::generate_full(cx, cz));
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let dt = start.elapsed().as_secs_f64();
    println!(
        "wall: {:.2} s  |  aggregate: {:.1} chunks/s  |  {:.1} ms/chunk wall-amortized",
        dt,
        total as f64 / dt,
        dt * 1e3 / total as f64
    );
}
