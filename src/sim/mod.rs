//! The simulation: a single `bevy_ecs` world ticked at 20 TPS on its own OS
//! thread.
//!
//! It owns all game state and never performs I/O — inbound packets arrive as
//! `ToSim` messages on the ingress channel and outbound packets leave through
//! per-player outboxes. Players are entities; the per-tick logic lives in
//! `systems`. See `docs/ARCHITECTURE.md`.

pub mod bridge;
mod components;
mod packets;
mod systems;

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
pub fn run(rx: tokio::sync::mpsc::Receiver<ToSim>, config: Arc<ServerConfig>) {
    let mut world = World::new();
    world.insert_resource(Ingress(Mutex::new(rx)));
    world.insert_resource(Config(config));
    world.insert_resource(Tick(0));
    // Entity id 1 is the first player; the framing test pins the first join to 1.
    world.insert_resource(NextEntityId(1));
    world.init_resource::<PlayerIndex>();
    world.init_resource::<Control>();

    let mut schedule = Schedule::default();
    schedule.add_systems(
        (
            systems::advance_tick,
            systems::drain_ingress,
            systems::keepalive,
        )
            .chain(),
    );

    info!("simulation started ({} TPS)", 1000 / TICK.as_millis());

    loop {
        let started = Instant::now();

        schedule.run(&mut world);

        if world.resource::<Control>().stop {
            info!("ingress closed; simulation stopping");
            return;
        }

        if let Some(rem) = TICK.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
}
