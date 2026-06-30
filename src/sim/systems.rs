//! The simulation systems, run once per tick in order:
//! `advance_tick` → `drain_ingress` → `keepalive`.
//!
//! `drain_ingress` is an exclusive system (`&mut World`) because it spawns and
//! despawns entities and fans chat out across every connection — work that
//! doesn't fit the parallel `Query` model. `keepalive` is an ordinary system.

use bevy_ecs::prelude::*;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::bridge::{Outbound, OutboxTx, Serverbound, ToSim};
use super::commands;
use super::components::*;
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

/// Advance the tick counter. Runs first so joins seed `last_tick` with the new
/// value and `keepalive` sees a consistent clock.
pub fn advance_tick(mut tick: ResMut<Tick>) {
    tick.0 += 1;
}

/// Drain every message the network delivered since the last tick and apply it.
pub fn drain_ingress(world: &mut World) {
    let mut msgs = Vec::new();
    let mut disconnected = false;
    {
        // Interior mutability via the `Mutex`; the `&Ingress` borrow ends with
        // this block so the message loop below can mutate the world freely.
        let ingress = world.resource::<Ingress>();
        let mut rx = ingress.0.lock().expect("ingress mutex poisoned");
        loop {
            match rx.try_recv() {
                Ok(m) => msgs.push(m),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }
    if disconnected {
        world.resource_mut::<Control>().stop = true;
    }
    for msg in msgs {
        match msg {
            ToSim::Joined { id, name, outbox } => on_joined(world, id, name, outbox),
            ToSim::Left { id } => on_left(world, id),
            ToSim::Packet { id, packet } => on_packet(world, id, packet),
        }
    }
}

fn on_joined(world: &mut World, id: Uuid, name: String, outbox: OutboxTx) {
    let entity_id = {
        let mut next = world.resource_mut::<NextEntityId>();
        let v = next.0;
        next.0 += 1;
        v
    };
    let (sx, sy, sz) = SPAWN;
    let join = world.resource::<Config>().join_params();

    // The whole join sequence flows through the outbox. If it overflows mid-burst
    // (slow or hostile client) drop the connection rather than register a player
    // who never received the world.
    if !send_join_sequence(&outbox, entity_id, sx, sy, sz, &join) {
        warn!(%name, "outbox full during join sequence; dropping");
        let _ = outbox.try_send(Outbound::Close);
        return;
    }

    let tick = world.resource::<Tick>().0;
    info!(%name, entity_id, "joined");
    let entity = world
        .spawn((
            PlayerId(id),
            Profile { name, entity_id },
            Pos {
                x: sx,
                y: sy,
                z: sz,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: false,
            },
            Conn { outbox },
            KeepAlive {
                id: 0,
                awaiting: false,
                last_tick: tick,
            },
        ))
        .id();
    world.resource_mut::<PlayerIndex>().0.insert(id, entity);
}

/// Build and push the join sequence. Ordering matters: the GameEvent puts the
/// client in its "waiting for chunks" state, the chunks satisfy that wait, then
/// the teleport settles the player. Returns `false` if any send fails.
fn send_join_sequence(
    outbox: &OutboxTx,
    entity_id: i32,
    sx: f64,
    sy: f64,
    sz: f64,
    join: &packets::JoinParams,
) -> bool {
    let mut ok = send(outbox, packets::play_login(entity_id, join));
    // Advertise the command tree right after login so the client highlights and
    // tab-completes our commands as it would against a vanilla server.
    ok &= send(outbox, commands::commands_packet());
    ok &= send(
        outbox,
        packets::game_event(packets::GAME_EVENT_LEVEL_CHUNKS_LOAD_START, 0.0),
    );
    ok &= send(outbox, packets::set_chunk_center(0, 0));
    // Stream exactly the advertised view distance of chunks so the client's
    // "Loading terrain" wait is fully satisfied.
    let radius = join.view_distance;
    for cx in -radius..=radius {
        for cz in -radius..=radius {
            ok &= send(outbox, packets::flat_chunk(cx, cz));
        }
    }
    ok &= send(
        outbox,
        packets::player_position(SPAWN_TELEPORT_ID, sx, sy, sz),
    );
    ok
}

fn on_left(world: &mut World, id: Uuid) {
    let entity = world.resource_mut::<PlayerIndex>().0.remove(&id);
    if let Some(e) = entity {
        let name = world.get::<Profile>(e).map(|p| p.name.clone());
        world.despawn(e);
        if let Some(name) = name {
            info!(%name, "left");
        }
    }
}

fn on_packet(world: &mut World, id: Uuid, packet: Serverbound) {
    let Some(&entity) = world.resource::<PlayerIndex>().0.get(&id) else {
        return; // packet for a player we've already dropped
    };
    match packet {
        Serverbound::Move {
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
        } => {
            if let Some(mut pos) = world.get_mut::<Pos>(entity) {
                if let Some(v) = x {
                    pos.x = v;
                }
                if let Some(v) = y {
                    pos.y = v;
                }
                if let Some(v) = z {
                    pos.z = v;
                }
                if let Some(v) = yaw {
                    pos.yaw = v;
                }
                if let Some(v) = pitch {
                    pos.pitch = v;
                }
                pos.on_ground = on_ground;
                debug!(x = pos.x, y = pos.y, z = pos.z, yaw = pos.yaw, pitch = pos.pitch, on_ground = pos.on_ground, "move");
            }
        }
        Serverbound::Chat(msg) => {
            let Some(name) = world.get::<Profile>(entity).map(|p| p.name.clone()) else {
                return;
            };
            info!(%name, message = %msg, "chat");
            let bytes = packets::system_chat(&format!("<{name}> {msg}"));
            // Fan out to every connection, including the sender.
            let mut q = world.query::<&Conn>();
            for conn in q.iter(world) {
                let _ = conn.outbox.try_send(Outbound::Packet(bytes.clone()));
            }
        }
        Serverbound::ChatCommand(line) => on_command(world, entity, &line),
        Serverbound::KeepAlive(echo) => {
            if let Some(mut ka) = world.get_mut::<KeepAlive>(entity) {
                if echo == ka.id {
                    ka.awaiting = false;
                }
            }
        }
        Serverbound::AcceptTeleport(tp) => {
            debug!(teleport_id = tp, "teleport confirmed");
        }
    }
}

/// Run a `/command` for `sender`. Like vanilla's `sendSuccess(..., false)`, the
/// reply goes only to the player who issued it; dispatch and the per-command
/// handlers live in `commands`.
fn on_command(world: &mut World, sender: Entity, line: &str) {
    let Some(name) = world.get::<Profile>(sender).map(|p| p.name.clone()) else {
        return;
    };
    info!(%name, command = %line, "command");

    let reply = commands::run(world, sender, line);
    let bytes = packets::system_chat_component(&reply);
    if let Some(conn) = world.get::<Conn>(sender) {
        let _ = conn.outbox.try_send(Outbound::Packet(bytes));
    }
}

/// Send keep-alives and disconnect anyone who missed the last one.
pub fn keepalive(
    tick: Res<Tick>,
    mut index: ResMut<PlayerIndex>,
    mut commands: Commands,
    mut players: Query<(Entity, &PlayerId, &Conn, &mut KeepAlive)>,
) {
    let now = tick.0;
    for (entity, pid, conn, mut ka) in players.iter_mut() {
        if now.saturating_sub(ka.last_tick) < KEEPALIVE_TICKS {
            continue;
        }
        if ka.awaiting {
            warn!(uuid = %pid.0, "keep-alive timeout");
            drop_player(&mut commands, &mut index, entity, pid.0, conn);
            continue;
        }
        ka.id = ka.id.wrapping_add(1);
        ka.awaiting = true;
        ka.last_tick = now;
        if conn
            .outbox
            .try_send(Outbound::Packet(packets::keep_alive(ka.id)))
            .is_err()
        {
            drop_player(&mut commands, &mut index, entity, pid.0, conn);
        }
    }
}

/// Ask a player's write task to close, then despawn the entity and forget it.
fn drop_player(
    commands: &mut Commands,
    index: &mut PlayerIndex,
    entity: Entity,
    uuid: Uuid,
    conn: &Conn,
) {
    let _ = conn.outbox.try_send(Outbound::Close);
    index.0.remove(&uuid);
    commands.entity(entity).despawn();
}

/// Push a framed packet to an outbox, reporting whether it was accepted.
fn send(outbox: &OutboxTx, bytes: bytes::Bytes) -> bool {
    outbox.try_send(Outbound::Packet(bytes)).is_ok()
}
