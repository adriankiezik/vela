//! Tick orchestration and connection health — the systems run once per tick.
//!
//! `advance_tick` bumps the clock, then `drain_ingress` applies everything the
//! network delivered since the last tick (dispatching to `player_lifecycle` and
//! `packet_handlers`), and `keepalive` polls liveness. The heavier per-tick work
//! lives in its own modules: world simulation in `world_tick`, movement
//! broadcasting in `movement`, and chunk streaming in `chunking`.
//!
//! `drain_ingress` is an exclusive system (`&mut World`) because it spawns and
//! despawns entities and fans chat out across every connection — work that
//! doesn't fit the parallel `Query` model. `keepalive` is an ordinary system.

use bevy_ecs::prelude::*;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::warn;
use uuid::Uuid;

use super::bridge::{Outbound, ToSim};
use super::components::*;
use super::packets;
use super::{packet_handlers, player_lifecycle};

/// Ticks between keep-alives. At 20 TPS, 200 ticks is 10 s — matching vanilla's
/// cadence. If a player hasn't echoed the previous one by the next interval it
/// is considered unresponsive and disconnected.
const KEEPALIVE_TICKS: u64 = 200;

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
            ToSim::Joined { id, name, outbox } => {
                player_lifecycle::on_joined(world, id, name, outbox)
            }
            ToSim::Left { id } => player_lifecycle::on_left(world, id),
            ToSim::Packet { id, packet } => packet_handlers::on_packet(world, id, packet),
        }
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
