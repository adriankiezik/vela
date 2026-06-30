//! ECS components and resources for the simulation `World`.
//!
//! A connected player is an entity carrying `PlayerId` + `Profile` + `Pos` +
//! `Conn` + `KeepAlive`. World-wide state lives in resources.

use std::collections::HashMap;
use std::sync::Mutex;

use bevy_ecs::prelude::*;
use uuid::Uuid;

use super::bridge::{OutboxTx, ToSim};

/// The player's stable identity, used to resolve incoming `ToSim` messages
/// (keyed by `Uuid`) back to this entity via `PlayerIndex`.
#[derive(Component)]
pub struct PlayerId(pub Uuid);

#[derive(Component)]
pub struct Profile {
    pub name: String,
    // Assigned at join and sent in the play-login packet. Read once entities are
    // tracked across players (AddEntity/RemoveEntities); held now so the id
    // space is owned in one place.
    #[allow(dead_code)]
    pub entity_id: i32,
}

/// Last-known position and orientation, updated from serverbound movement.
#[derive(Component)]
pub struct Pos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

/// The egress side of a player's connection — how the sim talks back. Cheap to
/// hold: a `tokio` mpsc sender.
#[derive(Component)]
pub struct Conn {
    pub outbox: OutboxTx,
}

/// Per-player keep-alive bookkeeping.
#[derive(Component)]
pub struct KeepAlive {
    pub id: i64,
    pub awaiting: bool,
    pub last_tick: u64,
}

/// The network ingress channel. Wrapped in a `Mutex` so the receiver (which is
/// `!Sync`) can live in a `Send + Sync` resource; the drain system is exclusive
/// and single-threaded, so the lock is always uncontended.
#[derive(Resource)]
pub struct Ingress(pub Mutex<tokio::sync::mpsc::Receiver<ToSim>>);

/// Monotonic tick counter (20 per second).
#[derive(Resource)]
pub struct Tick(pub u64);

/// Next entity id to hand to a joining player.
#[derive(Resource)]
pub struct NextEntityId(pub i32);

/// `Uuid` → `Entity` lookup for resolving inbound packets and disconnects.
#[derive(Resource, Default)]
pub struct PlayerIndex(pub HashMap<Uuid, Entity>);

/// Set when the ingress channel closes (server shutting down); the run loop
/// checks it and stops.
#[derive(Resource, Default)]
pub struct Control {
    pub stop: bool,
}
