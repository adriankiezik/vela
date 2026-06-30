//! The simulation: a single `World` ticked at 20 TPS on its own OS thread.
//!
//! It owns all game state and never performs I/O — inbound packets arrive as
//! `ToSim` messages and outbound packets leave through per-player outboxes.
//! See `docs/ARCHITECTURE.md`.

pub mod bridge;
mod packets;
mod world;

use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TryRecvError;
use tracing::info;

use bridge::ToSim;
use world::World;

/// One tick is 50 ms (20 TPS).
const TICK: Duration = Duration::from_millis(50);

/// Run the simulation loop until the ingress channel closes (all connections
/// gone and the listener dropped — i.e. shutdown). Blocks the calling thread;
/// spawn it on a dedicated OS thread, not a tokio worker.
pub fn run(mut rx: tokio::sync::mpsc::Receiver<ToSim>) {
    let mut world = World::new();
    info!("simulation started ({} TPS)", 1000 / TICK.as_millis());

    loop {
        let started = Instant::now();

        // Drain everything the network delivered since the last tick. Never
        // blocks: `try_recv` returns immediately when the queue is empty.
        loop {
            match rx.try_recv() {
                Ok(msg) => world.apply(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    info!("ingress closed; simulation stopping");
                    return;
                }
            }
        }

        world.tick();

        if let Some(rem) = TICK.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
}
