//! Phase split inside the NOISE stage (`ParityGenerator::fill_chunk`):
//! NoiseChunk setup vs density slice fills vs the per-block loop.
//! Run: cargo run --release --example profile_noise
use std::hint::black_box;

use vela::world::gen::density::{self, ParityGenerator};

fn run(label: &str, aquifers: bool, veins: bool) {
    let mut g = ParityGenerator::new_overworld(0x5EED_C0DE);
    g.random_state.settings.aquifers_enabled = aquifers;
    g.random_state.settings.ore_veins_enabled = veins;
    // Warm-up (template construction etc.).
    black_box(g.fill_chunk(9000, 9000));
    density::take_noise_fill_profile();

    let n = 32;
    for i in 0..n {
        black_box(g.fill_chunk(9000 + i, 9001));
    }
    let (setup, slices, blocks, chunks) = density::take_noise_fill_profile();
    let per = |d: std::time::Duration| d.as_secs_f64() * 1e3 / chunks as f64;
    println!("{label} ({chunks} chunks):");
    println!("  setup (NoiseChunk::for_chunk): {:7.3} ms/chunk", per(setup));
    println!("  slice fills (density corners): {:7.3} ms/chunk", per(slices));
    println!("  block loop                   : {:7.3} ms/chunk", per(blocks));
}

fn main() {
    run("full (aquifers + ore veins)", true, true);
    run("no ore veins", true, false);
    run("shape only (no aquifer, no veins)", false, false);

    // Per-status attribution over a steady-state serve strip.
    use vela::world::gen::pipeline::{self, ChunkStatus};
    for cx in 0..6 {
        black_box(pipeline::generate_full(cx, 5000));
    }
    pipeline::take_step_phase_profile();
    let n = 16;
    let t0 = std::time::Instant::now();
    for cx in 6..6 + n {
        black_box(pipeline::generate_full(cx, 5000));
    }
    let total = t0.elapsed();
    println!("\nserve strip: {:.2} ms/chunk", total.as_secs_f64() * 1e3 / n as f64);
    let steps = pipeline::take_step_phase_profile();
    let mut sum = std::time::Duration::ZERO;
    for (i, (d, c)) in steps.iter().enumerate() {
        if *c == 0 {
            continue;
        }
        sum += *d;
        println!(
            "  {:<32} {:7.2} ms/serve  ({} steps)",
            ChunkStatus::ALL[i].name(),
            d.as_secs_f64() * 1e3 / n as f64,
            c
        );
    }
    println!(
        "  {:<32} {:7.2} ms/serve",
        "unattributed (advance overhead)",
        (total - sum).as_secs_f64() * 1e3 / n as f64
    );
}
