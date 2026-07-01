//! The simulation: a single `bevy_ecs` world ticked at 20 TPS on its own OS
//! thread.
//!
//! It owns all game state and never performs I/O — inbound packets arrive as
//! `ToSim` messages on the ingress channel and outbound packets leave through
//! per-player outboxes. Players are entities; the per-tick logic lives in
//! `systems`. See `docs/ARCHITECTURE.md`.

pub mod bridge;
mod chat;
mod chunking;
mod commands;
mod components;
mod entity;
mod item_tick;
mod mob;
mod movement;
mod packet_handlers;
mod packets;
mod persistence;
mod player_lifecycle;
mod survival;
mod systems;
mod text;
mod world_tick;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use tracing::info;

use crate::config::ServerConfig;

use bridge::ToSim;
use components::{Config, Control, Ingress, NextEntityId, PlayerIndex, Tick};

/// One tick is 50 ms (20 TPS).
const TICK: Duration = Duration::from_millis(50);

/// Run the simulation loop until the ingress channel closes (all connections
/// gone and the listener dropped — i.e. shutdown). Blocks the calling thread;
/// spawn it on a dedicated OS thread, not a tokio worker.
pub fn run(
    rx: tokio::sync::mpsc::Receiver<ToSim>,
    config: Arc<ServerConfig>,
    shutdown: Arc<AtomicBool>,
) {
    let mut world = World::new();
    world.insert_resource(Ingress(Mutex::new(rx)));
    world.insert_resource(Config(config));
    world.insert_resource(Tick(0));
    // Entity id 1 is the first player; the framing test pins the first join to 1.
    world.insert_resource(NextEntityId(1));
    world.init_resource::<PlayerIndex>();
    // The network half raises this flag on Ctrl+C; the run loop watches it (and
    // the `/stop` command sets it) so an external shutdown saves the world.
    world.insert_resource(Control { stop: false, signal: shutdown });
    // World clock / weather / game rules (day-night cycle, weather, rules).
    world.init_resource::<world_tick::GameRules>();
    world.init_resource::<world_tick::WorldTime>();
    world.init_resource::<world_tick::Weather>();

    // Enable world persistence and load `level.dat` (world clock + game rules)
    // before the first tick, so a restarted world resumes where it left off.
    persistence::boot(&mut world);

    let mut schedule = Schedule::default();
    schedule.add_systems(
        (
            systems::advance_tick,
            systems::drain_ingress,
            // Survival: food drain / regen / void / starvation and the SetHealth
            // sync. Runs after drain_ingress so fall damage from this tick's moves
            // (applied in packet_handlers) is already reflected in the HUD sync.
            survival::survival_tick,
            world_tick::world_tick,
            // Dropped-item physics/pickup/merge/despawn. Runs after world_tick
            // (its own state) and before broadcast_movement; item entities emit
            // their own movement packets here rather than through the player-only
            // broadcast_movement path.
            item_tick::item_tick,
            // Living mobs: the natural spawner tops the world up (self-gated on the
            // clock), then the AI/physics pass wanders them and broadcasts their
            // movement (its own per-chunk fan-out, like item_tick).
            mob::mob_spawn,
            mob::mob_tick,
            movement::broadcast_movement,
            // Dynamic chunk streaming: must run after movement is applied so the
            // loaded-chunk set follows the player's current position this tick.
            chunking::stream_chunks,
            systems::keepalive,
        )
            .chain(),
    );

    info!("simulation started ({} TPS)", 1000 / TICK.as_millis());

    loop {
        let started = Instant::now();

        schedule.run(&mut world);

        // Periodic autosave (self-gates on the world clock).
        persistence::autosave(&mut world);

        if world.resource::<Control>().should_stop() {
            info!("shutdown requested; simulation stopping");
            persistence::shutdown(&mut world);
            return;
        }

        if let Some(rem) = TICK.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
}
