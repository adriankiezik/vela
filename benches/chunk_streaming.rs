//! How quickly do chunks appear for a client moving fast through the world?
//!
//! When a player moves, the server tracks their chunk position and — every time
//! they cross a chunk boundary — a leading strip of newly-visible chunks enters
//! view and must be produced (terrain generated, lit, and wire-encoded into a
//! `level_chunk` packet) before it can be streamed to the client. The wall-clock
//! cost of producing that strip *is* the "chunks appear slowly" latency a player
//! sees when flying. These benches measure exactly that pipeline.
//!
//! The real streaming path (`sim::chunking::stream_chunks` / `send_queued_chunks`)
//! is private to the binary and interleaves ECS state, network flow-control, and
//! background prefetch workers — none of which belongs in a CPU micro-benchmark.
//! So we drive the two public seams the streaming path itself calls into:
//!
//!   * [`world::chunk_columns`] — generate (if cold) + light + wire-encode a
//!     column. This is the client-ready product `send_queued_chunks` ships.
//!   * [`gen::GenChunk::generate`] — the raw terrain pass alone, as a baseline
//!     to show how much of the cost is generation vs. lighting/encoding.
//!
//! The view-distance geometry (`in_view` / the newly-visible set) mirrors
//! `sim::chunking` one-for-one (vanilla `ChunkTrackingView`, `bufferRange = 2`)
//! so the strip we generate is the exact set of chunks the live server would
//! enqueue on a boundary crossing.
//!
//! Interpreting the numbers: Criterion reports throughput in elements/sec, where
//! one element is one produced chunk — i.e. **chunks per second**. If the
//! pipeline sustains `T` chunks/sec at view distance `R`, then a player flying in
//! a straight line crosses one chunk boundary every `(2R+1)` chunks of work, so
//! the fastest they can move without the terrain lagging behind is roughly
//! `T / (2R+1)` chunk-boundaries per second — see benches/README.md for the full
//! derivation into blocks/sec.

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use vela::world::{self, gen::GenChunk};

/// A fixed seed so terrain cost is reproducible run-to-run. `set_seed` is a
/// one-time global set; the first bench to touch the world wins, which is fine —
/// every bench wants the same world.
const BENCH_SEED: i64 = 0x5EED_C0DE;

/// View distances to profile, in chunks. 8 is a conservative client, 16 is the
/// common default, 32 is a far-render client — the case that stresses streaming
/// hardest because each boundary crossing reveals the widest strip.
///
/// Note: parity worldgen here costs on the order of a second per cold column
/// single-threaded, so each larger view distance adds minutes to a run. Trim
/// this array (or filter on the command line) if you only care about one case.
const VIEW_DISTANCES: [i32; 3] = [8, 16, 32];

/// Global cursor pushing every sample into fresh, previously-ungenerated terrain.
/// Reusing coordinates would measure cache hits (the store keeps generated
/// columns resident) and warm gen-pipeline caches for that precise region —
/// understating the cost a player pays flying into *unseen* chunks. We stride far
/// enough that consecutive samples never overlap.
static X_CURSOR: AtomicI32 = AtomicI32::new(0);

/// One-time world seed init. Idempotent; safe to call from every bench.
fn ensure_seed() {
    world::set_seed(BENCH_SEED);
}

/// Reserve a fresh, non-overlapping band of the world for the next sample and
/// return its base chunk-X. `span` is how many chunks wide (in X) the sample
/// will touch; we advance the cursor by that plus a margin so no two samples
/// share terrain or pipeline-cache neighbourhoods.
fn fresh_base(span: i32) -> i32 {
    X_CURSOR.fetch_add(span + 8, Ordering::Relaxed)
}

/// Vanilla `ChunkTrackingView` membership with `bufferRange = 2` — identical to
/// `sim::chunking::in_view`. A chunk is in view iff, after shrinking each axis
/// gap by the 2-chunk buffer, it lies within `radius`.
fn in_view(center: (i32, i32), x: i32, z: i32, radius: i32) -> bool {
    let dx = ((x - center.0).abs() - 2).max(0) as i64;
    let dz = ((z - center.1).abs() - 2).max(0) as i64;
    dx * dx + dz * dz < (radius as i64) * (radius as i64)
}

/// The chunks that newly enter view when the tracked center moves `old -> new`,
/// nearest-first (the order `ChunkSender` prioritises). Mirrors the `added` half
/// of `sim::chunking::chunk_diff`.
fn newly_visible(old: (i32, i32), new: (i32, i32), radius: i32) -> Vec<(i32, i32)> {
    // in_view reaches at most radius+1 chunks off-axis, so radius+2 bounds the box.
    let reach = radius + 2;
    let mut added = Vec::new();
    for x in (new.0 - reach)..=(new.0 + reach) {
        for z in (new.1 - reach)..=(new.1 + reach) {
            if in_view(new, x, z, radius) && !in_view(old, x, z, radius) {
                added.push((x, z));
            }
        }
    }
    added.sort_by_key(|&(x, z)| {
        let dx = (x - new.0) as i64;
        let dz = (z - new.1) as i64;
        dx * dx + dz * dz
    });
    added
}

/// Produce (generate + light + wire-encode) every column in `coords`, then evict
/// them so the resident store — and its memory — stays bounded across samples.
/// Returns only the produce time; eviction is untimed teardown.
fn produce_and_reclaim(coords: &[(i32, i32)]) -> Duration {
    let start = Instant::now();
    for &(cx, cz) in coords {
        black_box(world::chunk_columns(cx, cz));
    }
    let elapsed = start.elapsed();
    for &(cx, cz) in coords {
        // game_time is only consulted for persistence bookkeeping, which is off
        // in the bench (no world dir); 0 is fine.
        world::evict_chunk(cx, cz, 0);
    }
    elapsed
}

/// Baseline: per-chunk cost of each pipeline stage on cold terrain.
///
///  * `generate`  — raw terrain only (`GenChunk::generate`).
///  * `full_wire` — terrain + lighting + wire encoding (`chunk_columns`), the
///    actual thing streamed to the client.
///
/// The gap between the two is what lighting + encoding add on top of generation.
fn bench_single_chunk(c: &mut Criterion) {
    ensure_seed();
    let mut group = c.benchmark_group("single_chunk");
    group.throughput(Throughput::Elements(1));

    group.bench_function("generate", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let cx = fresh_base(1);
                let start = Instant::now();
                black_box(GenChunk::generate(cx, 0));
                total += start.elapsed();
            }
            total
        });
    });

    group.bench_function("full_wire", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let cx = fresh_base(1);
                total += produce_and_reclaim(&[(cx, 0)]);
            }
            total
        });
    });

    group.finish();
}

/// The headline latency: producing the leading strip revealed by a *single*
/// chunk-boundary crossing into fresh terrain, per view distance. This is the
/// hitch a player feels the instant they cross into a new chunk — the client
/// cannot show those chunks until this work finishes. Throughput is the strip's
/// chunk count, so the report also gives cold chunks/sec.
fn bench_boundary_crossing(c: &mut Criterion) {
    ensure_seed();
    let mut group = c.benchmark_group("boundary_crossing");
    // Cold generation is heavy at large view distances; keep the run bounded.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));

    for &radius in &VIEW_DISTANCES {
        // The +X strip revealed by moving one chunk east. Size is stable across
        // positions, so compute it once for the throughput count.
        let strip_len = newly_visible((0, 0), (1, 0), radius).len();
        group.throughput(Throughput::Elements(strip_len as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(radius),
            &radius,
            |b, &radius| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let base = fresh_base(radius + 4);
                        let strip = newly_visible((base, 0), (base + 1, 0), radius);
                        total += produce_and_reclaim(&strip);
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_single_chunk, bench_boundary_crossing);
criterion_main!(benches);
