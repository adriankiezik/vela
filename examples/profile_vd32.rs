//! Whole-process RAM for a one-player, view-distance-32 loaded set: queue the
//! full 65x65 chunk square through the real prefetch worker pool (the same
//! path a player join warms), wait for every wire cache to build, then report
//! the store accounting and the process working set.
//!
//! Run: cargo run --release --example profile_vd32

use std::time::{Duration, Instant};

const RADIUS: i32 = 32;

fn main() {
    vela::world::set_seed(0x5EED_C0DE_i64);
    let side = 2 * RADIUS + 1;
    let want = (side * side) as usize;

    println!("prefetching {side}x{side} = {want} chunks through the worker pool...");
    let start = Instant::now();
    let mut coords = Vec::with_capacity(want);
    // Nearest-first ring order, like the live streamer.
    for r in 0..=RADIUS {
        for cz in -r..=r {
            for cx in -r..=r {
                if cx.abs().max(cz.abs()) == r {
                    coords.push((cx, cz));
                }
            }
        }
    }
    vela::world::prefetch(coords);

    loop {
        let s = vela::world::store_memory_stats();
        if s.wire_built >= want {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("all {want} chunks wire-ready in {:.1} s", start.elapsed().as_secs_f64());

    let s = vela::world::store_memory_stats();
    let total = s.baseline_bytes + s.heightmap_bytes + s.light_bytes + s.frame_bytes;
    let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
    println!("\nchunk store ({} chunks):", s.chunks);
    println!("  baselines:  {:8.1} MiB", mib(s.baseline_bytes));
    println!("  heightmaps: {:8.1} MiB", mib(s.heightmap_bytes));
    println!("  light:      {:8.1} MiB", mib(s.light_bytes));
    println!("  frames:     {:8.1} MiB", mib(s.frame_bytes));
    println!("  store total:{:8.1} MiB", mib(total));

    #[cfg(windows)]
    {
        let pid = std::process::id();
        if let Ok(out) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid}).WorkingSet64")])
            .output()
        {
            if let Ok(ws) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                println!(
                    "\nprocess working set: {:.1} MiB (store + pipeline worker caches + shared caches)",
                    ws as f64 / (1024.0 * 1024.0)
                );
            }
        }
    }
}
