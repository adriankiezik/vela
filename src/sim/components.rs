//! ECS components and resources for the simulation `World`.
//!
//! A connected player is an entity carrying `PlayerId` + `Profile` + `Pos` +
//! `Conn` + `KeepAlive`. World-wide state lives in resources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy_ecs::prelude::*;
use uuid::Uuid;

use super::bridge::{OutboxTx, ToSim};
use super::packets::JoinParams;
use crate::config::ServerConfig;

/// The player's stable identity, used to resolve incoming `ToSim` messages
/// (keyed by `Uuid`) back to this entity via `PlayerIndex`.
#[derive(Component)]
pub struct PlayerId(pub Uuid);

#[derive(Component)]
pub struct Profile {
    pub name: String,
    // Assigned at join, sent in the play-login packet, and used as the entity id
    // when this player is spawned for / moved on other players' clients.
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

/// Per-entity broadcast state, mirroring vanilla's `ServerEntity`: the position
/// and rotation last *sent* to tracking players. Movement packets carry deltas
/// relative to this base, and every observer shares one delta stream — so a
/// late observer's `AddEntity` is seeded from here to stay in sync.
#[derive(Component)]
pub struct Tracking {
    /// Last-sent position base (vanilla `VecDeltaCodec` base). Packet deltas are
    /// `round(cur * 4096) - round(base * 4096)`.
    pub base_x: f64,
    pub base_y: f64,
    pub base_z: f64,
    /// Last-sent packed angles (`Mth.packDegrees`: signed bytes).
    pub yaw: i8,
    pub pitch: i8,
    pub head: i8,
    /// Last-sent on-ground flag; a change forces a full position sync.
    pub on_ground: bool,
    /// Ticks since the last forced full sync (vanilla `teleportDelay`).
    pub teleport_delay: u32,
    /// Per-entity tick counter gating broadcast cadence (vanilla `tickCount`).
    pub tick_count: u32,
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

/// The loaded server configuration, shared with the network half. Held so the
/// join sequence can be built from `server.properties` (view distance, game
/// mode, max players, …).
#[derive(Resource)]
pub struct Config(pub Arc<ServerConfig>);

impl Config {
    /// The join-packet parameters derived from `server.properties`.
    pub fn join_params(&self) -> JoinParams {
        let p = &self.0.properties;
        JoinParams {
            max_players: p.max_players(),
            view_distance: p.view_distance(),
            simulation_distance: p.simulation_distance(),
            hardcore: p.hardcore(),
            online_mode: p.online_mode(),
            game_type: p.gamemode(),
        }
    }
}
