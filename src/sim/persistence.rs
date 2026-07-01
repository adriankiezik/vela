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
    storage::init(&level_name);
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
            info!(
                level = %data.level_name,
                seed = data.seed,
                game_time = data.game_time,
                "loaded level.dat"
            );
            apply(world, &data);
        }
        None => {
            info!(level = %level_name, "no level.dat; creating a new world");
            save_level_dat(world);
        }
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
    info!("world saved on shutdown");
}

/// Apply a loaded `level.dat` to the live world clock and game rules. Seed and
/// spawn are persisted but not fed back (generation stays on the fixed terrain
/// seed and the spawn is the origin column — see the module notes).
fn apply(world: &mut World, data: &LevelData) {
    {
        let mut time = world.resource_mut::<WorldTime>();
        time.game_time = data.game_time;
        time.day_time = data.day_time;
    }
    world.resource_mut::<GameRules>().apply_saved(&data.game_rules);
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

    // Spawn is the top of the origin column, matching the join sequence.
    let spawn_y = crate::world::surface_height(0, 0) + 1;
    let mut data = LevelData::new(level_name, crate::world::SEED as i64, spawn_y);
    data.game_time = time.game_time;
    data.day_time = time.day_time;
    data.game_rules = rules.to_saved();

    if let Err(e) = data.save(&path) {
        warn!(error = %e, "failed to write level.dat");
    }
}
