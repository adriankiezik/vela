# Chunk-streaming benchmarks

`chunk_streaming.rs` measures **how quickly chunks appear for a client moving
fast through the world** — i.e. how fast the server can produce the newly-visible
chunks that a player reveals as they cross chunk boundaries.

## Running

```sh
cargo bench --bench chunk_streaming
```

Filter to one group or view distance:

```sh
cargo bench --bench chunk_streaming -- single_chunk
cargo bench --bench chunk_streaming -- boundary_crossing/16
```

HTML reports (with plots) land in `target/criterion/`.

> **Heads-up — this is a slow bench.** The benches generate real, cold parity
> terrain, which costs on the order of **~1 second per column single-threaded**
> in this codebase (the worldgen is deliberately vanilla-accurate; see the
> `[profile.test]` note in `Cargo.toml`). A single `boundary_crossing/32` sample
> alone generates ~65 columns, so a full run across all three view distances
> takes on the order of ten-plus minutes. Filter to one view distance while
> iterating, or trim `VIEW_DISTANCES` in the bench.

## What each group measures

| Group | What it does | Answers |
|-------|--------------|---------|
| `single_chunk/generate` | One `GenChunk::generate` on fresh terrain | Raw terrain cost per column |
| `single_chunk/full_wire` | One `world::chunk_columns` (generate + light + wire-encode) | Cost of the client-ready column; gap vs. `generate` is lighting + encoding |
| `boundary_crossing/{R}` | Produce the whole strip revealed by **one** boundary crossing into fresh terrain, at view distance `R` | The hitch the moment a player crosses into a new chunk, and cold chunks/sec |

Throughput is reported in **elements/sec = chunks/sec** (one element = one
produced column). Every sample runs on fresh coordinates and evicts what it
generated, so figures reflect real generation, never store cache hits.

Note on realism: column generation is independent of what's already resident —
`GenChunk::generate` reads no neighbours from the store, only the gen pipeline's
own thread-local caches. So keeping a player's trailing chunks loaded does *not*
speed up the leading edge; the per-column cost above is what the streamer pays
whether the client is standing still or flying. That's why one crossing
characterises the movement cost fully.

## Turning chunks/sec into a max fly speed

A player moving in a straight line reveals a strip of `≈ 2R+1` new columns each
time they cross one chunk boundary (16 blocks). The live server generates these
across a pool of `chunk-prefetch` workers — `available_parallelism() - 2` of them
(see `world::prefetch`) — so aggregate throughput is roughly the single-column
rate times the worker count `W`. If a column costs `t` seconds:

```
aggregate chunks/sec  =  W / t
boundaries/sec        =  (W / t) / (2R + 1)
blocks/sec            =  16 · (W / t) / (2R + 1)
```

Worked example — plug in a measured `t ≈ 1.0 s/column`, a 12-core host
(`W = 10`), render distance `R = 16` (`2R + 1 = 33`):

```
aggregate     = 10 / 1.0    = 10 columns/sec
boundaries/s  = 10 / 33     ≈ 0.30 crossings/sec
blocks/s      = 16 · 0.30   ≈ 4.8 blocks/second
```

So on that host at render distance 16 the terrain roughly keeps pace with
**walking/sprinting (~4.3–5.6 b/s)** but a client flying faster than that would
out-run generation and see chunks pop in behind them. Halving `t` or doubling
`W` doubles the sustainable speed; widening render distance (larger `2R + 1`)
shrinks it. Run the bench to get your host's real `t`, then plug it into the
formula for the speed at which chunks start "appearing slowly."

## How it stays honest

- View geometry (`in_view`, the newly-visible set) mirrors `sim::chunking`
  exactly (vanilla `ChunkTrackingView`, `bufferRange = 2`), so the strips
  generated are the precise set the live streamer enqueues on a crossing.
- Every sample runs on **fresh** world coordinates and evicts what it generated,
  so measurements reflect real generation — never store cache hits.
- Only generation/encoding is timed; coordinate math and eviction are untimed
  setup/teardown (`iter_custom`).
