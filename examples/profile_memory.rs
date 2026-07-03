//! Memory profiler for the resident chunk store: generates a square of chunks
//! through the live `chunk_columns` path (baseline + wire cache), reports the
//! per-chunk byte breakdown, and extrapolates to a view-distance-32 loaded set
//! (65×65 = 4225 chunks around one player).
//!
//! Run: cargo run --release --example profile_memory
//! Optionally set VELA_PROFILE_RADIUS (chunk radius of the generated square,
//! default 6 → 13×13 = 169 chunks).

use std::time::Instant;

/// Chunks a single player holds resident at view distance 32.
const VIEW_32_CHUNKS: usize = 65 * 65;

fn main() {
    let seed = 0x5EED_C0DE_i64;
    vela::world::set_seed(seed);

    let radius: i32 = std::env::var("VELA_PROFILE_RADIUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let side = 2 * radius + 1;
    let count = (side * side) as usize;

    println!("generating {side}x{side} = {count} chunks (radius {radius})...");
    let start = Instant::now();
    for cz in -radius..=radius {
        for cx in -radius..=radius {
            std::hint::black_box(vela::world::chunk_columns(cx, cz));
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "generated in {:.1} s  ({:.2} ms/chunk single-threaded)\n",
        elapsed,
        elapsed * 1e3 / count as f64
    );

    let s = vela::world::store_memory_stats();
    let per = |bytes: usize| bytes as f64 / s.chunks as f64 / 1024.0;
    println!("resident chunks: {} (wire built: {})", s.chunks, s.wire_built);
    println!("per-chunk averages:");
    println!("  baseline (GenChunk grid): {:8.1} KiB", per(s.baseline_bytes));
    println!("  heightmaps:               {:8.1} KiB", per(s.heightmap_bytes));
    println!("  light sections (heap):    {:8.1} KiB", per(s.light_bytes));
    println!("  level_chunk frame:        {:8.1} KiB (blob is a slice of it)", per(s.frame_bytes));

    let per_chunk_total = (s.baseline_bytes + s.heightmap_bytes + s.light_bytes + s.frame_bytes)
        as f64
        / s.chunks as f64;
    println!("\nper-chunk resident total:      {:8.1} KiB", per_chunk_total / 1024.0);
    println!(
        "\nview distance 32 extrapolation ({VIEW_32_CHUNKS} chunks, one player): {:8.1} MiB",
        per_chunk_total * VIEW_32_CHUNKS as f64 / (1024.0 * 1024.0)
    );

    // Whole-process working set (includes the thread-local pipeline proto
    // caches and shared carver/write caches the store accounting can't see).
    #[cfg(windows)]
    {
        let pid = std::process::id();
        if let Ok(out) = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {pid}).WorkingSet64"),
            ])
            .output()
        {
            if let Ok(ws) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                println!("\nprocess working set: {:.1} MiB", ws as f64 / (1024.0 * 1024.0));
            }
        }
    }
}
