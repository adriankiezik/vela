//! Per-feature + serve-phase attribution for the FEATURES serve path.
//! Run: cargo run --release --example profile_features
use std::hint::black_box;
use std::time::Instant;

use vela::world::gen::{features, pipeline};

fn main() {
    // Warm the thread-local pipeline on a fresh strip, then drain the tallies so
    // the measured window excludes warm-up.
    for cx in 0..6 {
        black_box(pipeline::generate_full(cx, 3000));
    }
    let _ = features::take_feature_profile();
    let _ = pipeline::take_serve_phase_profile();

    // Measured strip: steady-state marginal serves.
    let n = 16;
    let t0 = Instant::now();
    for cx in 6..6 + n {
        black_box(pipeline::generate_full(cx, 3000));
    }
    let total = t0.elapsed();
    println!("measured {n} serves in {:.1} ms  ({:.2} ms/chunk)", total.as_secs_f64() * 1e3, total.as_secs_f64() * 1e3 / n as f64);

    let phases = pipeline::take_serve_phase_profile();
    let names = ["advance->carvers", "3x3 clone", "decorate", "diff"];
    println!("\nserve phases (per measured chunk):");
    for (i, (d, c)) in phases.iter().enumerate() {
        println!(
            "  {:<18} {:8.2} ms/chunk  ({} calls, {:.2} ms total)",
            names[i],
            d.as_secs_f64() * 1e3 / n as f64,
            c,
            d.as_secs_f64() * 1e3
        );
    }

    let mut feats = features::take_feature_profile();
    feats.sort_by(|a, b| b.1.cmp(&a.1));
    let total_feat: f64 = feats.iter().map(|(_, d, _)| d.as_secs_f64()).sum();
    println!("\nfeature time total: {:.2} ms/chunk over {} distinct features", total_feat * 1e3 / n as f64, feats.len());
    println!("top 40 features by cumulative time (per measured chunk):");
    for (id, d, c) in feats.iter().take(40) {
        println!(
            "  {:9.3} ms/chunk  {:7} calls  {}",
            d.as_secs_f64() * 1e3 / n as f64,
            c,
            id
        );
    }
}
