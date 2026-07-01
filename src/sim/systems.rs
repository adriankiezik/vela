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
            ToSim::Joined {
                id,
                name,
                outbox,
                view_distance,
            } => player_lifecycle::on_joined(world, id, name, outbox, view_distance),
            ToSim::Left { id } => player_lifecycle::on_left(world, id),
            ToSim::Packet { id, packet } => packet_handlers::on_packet(world, id, packet),
        }
    }
}

/// Force-disconnect players whose outbox overflowed on an ordering-critical
/// clientbound packet (flagged by [`Conn::send_reliable`]). Silently dropping a
/// `set_chunk_center` / `forget_chunk` / chunk-batch frame corrupts the client's
/// view irrecoverably; vanilla treats an unrecoverable connection overload as a
/// disconnect, not silent corruption, so we kick the lagging player cleanly.
///
/// Exclusive because it despawns entities and fans a removal packet across every
/// remaining connection. Reuses the normal [`player_lifecycle::despawn_player`]
/// teardown, so chunk-eviction refcounts (`ChunkRefs::release`) and the entity
/// despawn run exactly once — the same balanced path a clean `Left` takes. Like
/// the keep-alive `drop_player` path, `id` is removed from `PlayerIndex` first,
/// so the eventual `ToSim::Left` (raised when the write task observes the dropped
/// outbox sender) finds nothing and `on_left` no-ops — no double release.
pub fn drop_lagging_players(world: &mut World) {
    let doomed: Vec<(Entity, Uuid)> = {
        let mut q = world.query::<(Entity, &PlayerId, &Conn)>();
        q.iter(world)
            .filter(|(_, _, conn)| conn.disconnect_requested())
            .map(|(entity, pid, _)| (entity, pid.0))
            .collect()
    };
    for (entity, uuid) in doomed {
        warn!(%uuid, "client fell too far behind (outbox full on ordering-critical packet); disconnecting");
        // Best-effort prompt close; if the outbox is full this fails and dropping
        // the sender (in `despawn`) is what actually ends the write task.
        if let Some(conn) = world.get::<Conn>(entity) {
            let _ = conn.outbox.try_send(Outbound::Close);
        }
        world.resource_mut::<PlayerIndex>().0.remove(&uuid);
        player_lifecycle::despawn_player(world, entity, uuid);
    }
}

/// Send keep-alives and flag anyone who missed the last one for disconnect.
///
/// A timeout (or an overflowed keep-alive send) no longer tears the player down
/// inline; it flags the connection with [`Conn::request_disconnect`] and lets the
/// exclusive [`drop_lagging_players`] — which runs immediately after this system
/// in the same tick — perform the unified teardown. That routes through
/// [`player_lifecycle::despawn_player`], which saves the player's data, releases
/// chunk references, and broadcasts the player-info-remove + remove-entities to
/// every other client, exactly as a clean disconnect does.
pub fn keepalive(
    tick: Res<Tick>,
    mut players: Query<(&PlayerId, &Conn, &mut KeepAlive)>,
) {
    let now = tick.0;
    for (pid, conn, mut ka) in players.iter_mut() {
        if now.saturating_sub(ka.last_tick) < KEEPALIVE_TICKS {
            continue;
        }
        if ka.awaiting {
            warn!(uuid = %pid.0, "keep-alive timeout");
            conn.request_disconnect();
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
            conn.request_disconnect();
        }
    }
}
