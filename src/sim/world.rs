//! The placeholder game world: a flat `HashMap` of players.
//!
//! This is deliberately the simplest thing that exercises the full bridge —
//! join sequence, chat fan-out, movement tracking, keep-alive — without an ECS.
//! Per `docs/ARCHITECTURE.md` step 3, this struct is what `bevy_ecs` replaces
//! once the channel boundary is proven; `net` never has to change for that swap.

use std::collections::HashMap;

use tracing::{debug, info, warn};
use uuid::Uuid;

use super::bridge::{Outbound, OutboxTx, Serverbound};
use super::packets;

/// Ticks between keep-alives. At 20 TPS, 200 ticks is 10 s — matching vanilla's
/// cadence. If a player hasn't echoed the previous one by the next interval it
/// is considered unresponsive and disconnected.
const KEEPALIVE_TICKS: u64 = 200;

/// Spawn point every player is teleported to on join.
const SPAWN: (f64, f64, f64) = (0.0, 64.0, 0.0);
/// Teleport id for the initial spawn synchronization; the client echoes it back
/// via `AcceptTeleportation`.
const SPAWN_TELEPORT_ID: i32 = 1;

/// A connected player's authoritative server-side record.
struct Player {
    name: String,
    // Assigned at join and sent in the play-login packet. Will be referenced
    // once entities are tracked across players (AddEntity/RemoveEntities); held
    // now so the id space is owned in one place.
    #[allow(dead_code)]
    entity_id: i32,
    outbox: OutboxTx,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    // Keep-alive bookkeeping.
    keepalive_id: i64,
    awaiting_keepalive: bool,
    last_keepalive_tick: u64,
}

impl Player {
    /// Push a framed packet to this player's write task. Returns `false` if the
    /// outbox is full or gone — the client can't keep up, so the caller should
    /// disconnect it.
    fn send(&self, bytes: bytes::Bytes) -> bool {
        self.outbox.try_send(Outbound::Packet(bytes)).is_ok()
    }
}

pub struct World {
    players: HashMap<Uuid, Player>,
    next_entity_id: i32,
    tick: u64,
}

impl World {
    pub fn new() -> Self {
        Self {
            players: HashMap::new(),
            // Entity id 1 is the first player; vanilla reserves nothing special
            // here, but the framing test pins the first join to id 1.
            next_entity_id: 1,
            tick: 0,
        }
    }

    /// Apply one inbound message from the network layer.
    pub fn apply(&mut self, msg: super::bridge::ToSim) {
        use super::bridge::ToSim;
        match msg {
            ToSim::Joined { id, name, outbox } => self.on_joined(id, name, outbox),
            ToSim::Left { id } => self.on_left(id),
            ToSim::Packet { id, packet } => self.on_packet(id, packet),
        }
    }

    /// Advance the simulation by one tick. Currently only drives keep-alives.
    pub fn tick(&mut self) {
        self.tick += 1;

        let now = self.tick;
        let mut timed_out: Vec<Uuid> = Vec::new();
        for (id, p) in self.players.iter_mut() {
            if now.saturating_sub(p.last_keepalive_tick) < KEEPALIVE_TICKS {
                continue;
            }
            if p.awaiting_keepalive {
                warn!(name = %p.name, "keep-alive timeout");
                timed_out.push(*id);
                continue;
            }
            p.keepalive_id = p.keepalive_id.wrapping_add(1);
            p.awaiting_keepalive = true;
            p.last_keepalive_tick = now;
            if !p.send(packets::keep_alive(p.keepalive_id)) {
                timed_out.push(*id);
            }
        }
        for id in timed_out {
            self.disconnect(id);
        }
    }

    fn on_joined(&mut self, id: Uuid, name: String, outbox: OutboxTx) {
        let entity_id = self.next_entity_id;
        self.next_entity_id += 1;
        let (sx, sy, sz) = SPAWN;

        // The join sequence. Ordering matters: the GameEvent puts the client in
        // its "waiting for chunks" state, then the chunks satisfy that wait, then
        // the teleport settles the player. All of it flows through the outbox.
        let mut ok = outbox
            .try_send(Outbound::Packet(packets::play_login(entity_id)))
            .is_ok();
        ok &= outbox
            .try_send(Outbound::Packet(packets::game_event(
                packets::GAME_EVENT_LEVEL_CHUNKS_LOAD_START,
                0.0,
            )))
            .is_ok();
        ok &= outbox
            .try_send(Outbound::Packet(packets::set_chunk_center(0, 0)))
            .is_ok();
        for cx in -packets::VIEW_RADIUS..=packets::VIEW_RADIUS {
            for cz in -packets::VIEW_RADIUS..=packets::VIEW_RADIUS {
                ok &= outbox
                    .try_send(Outbound::Packet(packets::empty_chunk(cx, cz)))
                    .is_ok();
            }
        }
        ok &= outbox
            .try_send(Outbound::Packet(packets::player_position(
                SPAWN_TELEPORT_ID,
                sx,
                sy,
                sz,
            )))
            .is_ok();

        if !ok {
            // Outbox overflowed mid-join (slow or hostile client). Drop it
            // rather than register a player who never received the world.
            warn!(%name, "outbox full during join sequence; dropping");
            let _ = outbox.try_send(Outbound::Close);
            return;
        }

        info!(%name, entity_id, "joined");
        self.players.insert(
            id,
            Player {
                name,
                entity_id,
                outbox,
                x: sx,
                y: sy,
                z: sz,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: false,
                keepalive_id: 0,
                awaiting_keepalive: false,
                last_keepalive_tick: self.tick,
            },
        );
    }

    fn on_left(&mut self, id: Uuid) {
        if let Some(p) = self.players.remove(&id) {
            info!(name = %p.name, "left");
        }
    }

    fn on_packet(&mut self, id: Uuid, packet: Serverbound) {
        match packet {
            Serverbound::Move {
                x,
                y,
                z,
                yaw,
                pitch,
                on_ground,
            } => {
                if let Some(p) = self.players.get_mut(&id) {
                    if let Some(v) = x {
                        p.x = v;
                    }
                    if let Some(v) = y {
                        p.y = v;
                    }
                    if let Some(v) = z {
                        p.z = v;
                    }
                    if let Some(v) = yaw {
                        p.yaw = v;
                    }
                    if let Some(v) = pitch {
                        p.pitch = v;
                    }
                    p.on_ground = on_ground;
                    debug!(name = %p.name, x = p.x, y = p.y, z = p.z, yaw = p.yaw, pitch = p.pitch, on_ground, "move");
                }
            }
            Serverbound::Chat(msg) => {
                let Some(name) = self.players.get(&id).map(|p| p.name.clone()) else {
                    return;
                };
                info!(%name, message = %msg, "chat");
                self.broadcast(&packets::system_chat(&format!("<{name}> {msg}")));
            }
            Serverbound::KeepAlive(echo) => {
                if let Some(p) = self.players.get_mut(&id) {
                    if echo == p.keepalive_id {
                        p.awaiting_keepalive = false;
                    }
                }
            }
            Serverbound::AcceptTeleport(tp) => {
                debug!(teleport_id = tp, "teleport confirmed");
            }
        }
    }

    /// Send a framed packet to every connected player.
    fn broadcast(&self, bytes: &bytes::Bytes) {
        for p in self.players.values() {
            let _ = p.send(bytes.clone());
        }
    }

    /// Tear down a player: ask its write task to close, then forget it.
    fn disconnect(&mut self, id: Uuid) {
        if let Some(p) = self.players.remove(&id) {
            let _ = p.outbox.try_send(Outbound::Close);
            info!(name = %p.name, "disconnected");
        }
    }
}
