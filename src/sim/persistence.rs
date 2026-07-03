//! World save/load wiring for the simulation.
//!
//! This is the thin bridge between the simulation's live state (the world clock,
//! game rules, and the chunk store) and the on-disk persistence layer in
//! [`crate::world::storage`]. It runs at three points in the server lifecycle:
//!   * [`boot`] — enable persistence for the configured `level-name`, then load
//!     `level.dat` (applying the saved clock + game rules) or create a fresh one;
//!   * [`autosave`] — periodically flush dirty chunks and rewrite `level.dat`;
//!   * [`shutdown`] — a final save before the tick loop exits.
//!
//! Chunk load/save itself is handled lazily inside the chunk store
//! (`chunk_data`): a chunk loads from its region file on first touch and dirty
//! chunks are written by [`crate::world::save_dirty_chunks`].

use std::sync::atomic::{AtomicU64, Ordering};

use bevy_ecs::prelude::*;
use tracing::{info, warn};

use crate::world::storage::{self, LevelData};

use super::components::Config;
use super::world_tick::{GameRules, WorldTime};

/// Autosave cadence in ticks. Vanilla's dedicated server flushes every 6000 ticks
/// (5 minutes at 20 TPS); we match it.
pub const AUTOSAVE_INTERVAL: u64 = 6000;

/// Enable persistence for the configured world and load `level.dat`. On a fresh
/// world (no `level.dat`) the current defaults are written straight back so the
/// file exists from the first boot.
pub fn boot(world: &mut World) {
    let level_name = world
        .resource::<Config>()
        .0
        .properties
        .level_name()
        .to_string();
    // Root the world under the runtime dir (CWD for a shipped binary, the
    // `target/…` exe dir under `cargo run`) so a dev run keeps the generated
    // `world/` inside `target/` instead of next to `src/world`.
    storage::init(crate::runtime::dir().join(&level_name));
    if !storage::is_enabled() {
        info!("world persistence disabled (could not open save directory)");
        return;
    }

    match storage::level_dat_path().and_then(|p| match LevelData::load(&p) {
        Ok(data) => data,
        Err(e) => {
            warn!(error = %e, "failed to read level.dat; starting a fresh world");
            None
        }
    }) {
        Some(data) => {
            // An existing level.dat always wins: its persisted seed is threaded
            // into generation so the world regenerates reproducibly (must happen
            // before any chunk is touched). `level-seed` is ignored here.
            let seed = resolve_seed(Some(data.seed), None, false);
            info!(
                level = %data.level_name,
                seed,
                game_time = data.game_time,
                "loaded level.dat"
            );
            crate::world::set_seed(seed);
            apply(world, &data);
        }
        None => {
            // Fresh world: honour `level-seed` from server.properties, matching
            // vanilla `DedicatedServerProperties`
            // (`WorldOptions.parseSeed(levelSeed).orElse(randomSeed())`).
            let level_seed = world.resource::<Config>().0.properties.level_seed();
            let seed = resolve_seed(None, level_seed, fixed_seed_requested());
            info!(level = %level_name, seed, "no level.dat; creating a new world");
            crate::world::set_seed(seed);
            save_level_dat(world);
        }
    }
}

/// Whether `VELA_FIXED_SEED` is set — a determinism escape hatch. When present,
/// a fresh world with an *empty* `level-seed` pins its seed to
/// [`crate::world::DEFAULT_SEED`] (a reproducible dev world) instead of the
/// vanilla random seed. An explicit `level-seed` always takes precedence over it.
fn fixed_seed_requested() -> bool {
    std::env::var_os("VELA_FIXED_SEED").is_some()
}

/// Resolve the world seed at boot, mirroring the vanilla dedicated-server flow:
///   * a loaded `level.dat` seed (`loaded`) always wins;
///   * otherwise a parsed `level-seed` (`level_seed`) is honoured;
///   * an empty `level-seed` yields a random seed, unless `fixed` pins it to
///     [`crate::world::DEFAULT_SEED`] for reproducible dev worlds.
///
/// Pure and total so the precedence matrix is unit-testable without a live world.
fn resolve_seed(loaded: Option<i64>, level_seed: Option<i64>, fixed: bool) -> i64 {
    match loaded {
        Some(seed) => seed,
        None => match level_seed {
            Some(seed) => seed,
            None if fixed => crate::world::DEFAULT_SEED as i64,
            None => random_seed(),
        },
    }
}

/// A random world seed for an empty `level-seed`, vanilla's
/// `WorldOptions.randomSeed()` (`RandomSource.create().nextLong()`). Any entropy
/// source satisfies the spec; we splitmix64 the wall clock into a 64-bit value.
fn random_seed() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut h = nanos ^ 0x9E37_79B9_7F4A_7C15;
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    h as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_dat_seed_wins_over_level_seed_property() {
        // A loaded level.dat seed takes precedence over any `level-seed` and
        // over the fixed-seed escape hatch.
        assert_eq!(resolve_seed(Some(777), Some(42), false), 777);
        assert_eq!(resolve_seed(Some(777), None, true), 777);
        assert_eq!(resolve_seed(Some(-1), None, false), -1);
    }

    #[test]
    fn fresh_world_honours_level_seed() {
        // No level.dat: the parsed `level-seed` is used verbatim.
        assert_eq!(resolve_seed(None, Some(12345), false), 12345);
        assert_eq!(resolve_seed(None, Some(0), false), 0);
    }

    #[test]
    fn fresh_world_empty_seed_is_random_or_fixed() {
        // Empty `level-seed` with the escape hatch → DEFAULT_SEED (reproducible).
        assert_eq!(resolve_seed(None, None, true), crate::world::DEFAULT_SEED as i64);
        // Without it → a random seed; two draws differ with overwhelming odds and
        // it is never accidentally DEFAULT_SEED. (Deterministic assertion: the
        // clock advances between calls, so the splitmix outputs diverge.)
        let a = random_seed();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = random_seed();
        assert_ne!(a, b, "random seeds should differ across time");
    }
}

/// Ticks elapsed since the last autosave. Tracked here (rather than gating on
/// `game_time % INTERVAL`) so the save fires strictly every `AUTOSAVE_INTERVAL`
/// elapsed ticks — never at boot (`game_time == 0`) and never every tick if the
/// world clock is frozen (e.g. `doDaylightCycle`/paused time).
static TICKS_SINCE_SAVE: AtomicU64 = AtomicU64::new(0);

/// Periodic save: flush dirty chunks and rewrite `level.dat`, gated on the
/// autosave interval. Safe to call every tick — it self-gates and no-ops when
/// persistence is disabled.
pub fn autosave(world: &mut World) {
    if !storage::is_enabled() {
        return;
    }
    if TICKS_SINCE_SAVE.fetch_add(1, Ordering::Relaxed) + 1 < AUTOSAVE_INTERVAL {
        return;
    }
    TICKS_SINCE_SAVE.store(0, Ordering::Relaxed);
    let game_time = world.resource::<WorldTime>().game_time;
    crate::world::save_dirty_chunks(game_time);
    save_level_dat(world);
    super::player_lifecycle::save_all_players(world);
    info!(game_time, "autosaved world");
}

/// Final save on shutdown: persist all dirty chunks and `level.dat`.
pub fn shutdown(world: &mut World) {
    if !storage::is_enabled() {
        return;
    }
    let game_time = world.resource::<WorldTime>().game_time;
    crate::world::save_dirty_chunks(game_time);
    save_level_dat(world);
    super::player_lifecycle::save_all_players(world);
    info!("world saved on shutdown");
}

/// Apply a loaded `level.dat` to the live world clock and game rules. The seed is
/// fed back into generation by the caller (`boot`); spawn is re-derived from the
/// generator each join.
fn apply(world: &mut World, data: &LevelData) {
    {
        let mut time = world.resource_mut::<WorldTime>();
        time.game_time = data.game_time;
        time.day_time = data.day_time;
    }
    world
        .resource_mut::<GameRules>()
        .apply_saved(&data.game_rules);
}

/// Write `level.dat` from the current clock, game rules, and the origin spawn.
fn save_level_dat(world: &World) {
    let Some(path) = storage::level_dat_path() else {
        return;
    };
    let level_name = world
        .resource::<Config>()
        .0
        .properties
        .level_name()
        .to_string();
    let time = world.resource::<WorldTime>();
    let rules = world.resource::<GameRules>();

    // Spawn is the top of a solid, dry column near origin, matching the join
    // sequence's spawn selection. `world_spawn()` is memoised, so this save path
    // (autosave runs periodically on the tick thread) no longer re-spirals the
    // `surface_height` search on every write — it reads the cached column. The
    // single `surface_height` for the y is a resident peek once the spawn area is
    // loaded; it may generate one chunk only when saving with no player nearby.
    let (sx, sz) = crate::world::world_spawn();
    let spawn_y = crate::world::surface_height(sx, sz) + 1;
    let mut data = LevelData::new(level_name, crate::world::seed() as i64, spawn_y);
    data.spawn_x = sx;
    data.spawn_z = sz;
    data.game_time = time.game_time;
    data.day_time = time.day_time;
    data.game_rules = rules.to_saved();

    if let Err(e) = data.save(&path) {
        warn!(error = %e, "failed to write level.dat");
    }
}
