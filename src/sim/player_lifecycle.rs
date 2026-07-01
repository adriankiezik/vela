//! Player join/leave lifecycle: building and pushing the join sequence, syncing
//! world state to a newcomer, announcing arrivals/departures to everyone else,
//! and spawning/despawning the player entity. Driven by `systems::drain_ingress`.

use bevy_ecs::prelude::*;
use tracing::{info, warn};
use uuid::Uuid;

use super::bridge::{Outbound, OutboxTx};
use super::chunking::in_view;
use super::commands;
use super::components::*;
use super::packets;

/// Spawn column (X/Z). The Y is derived per-join from the generated surface
/// height (see [`on_joined`]) so the player lands on top of the terrain rather
/// than inside it.
const SPAWN_XZ: (f64, f64) = (0.0, 0.0);
/// Teleport id for the initial spawn synchronization; the client echoes it back
/// via `AcceptTeleportation`.
const SPAWN_TELEPORT_ID: i32 = 1;

/// A snapshot of an already-online player, taken when someone new joins so the
/// newcomer can be told who is already here (and vice versa).
struct Existing {
    uuid: Uuid,
    name: String,
    entity_id: i32,
    base_x: f64,
    base_y: f64,
    base_z: f64,
    yaw: i8,
    pitch: i8,
    head: i8,
    sneaking: bool,
    sprinting: bool,
    outbox: OutboxTx,
}

pub(super) fn on_joined(world: &mut World, id: Uuid, name: String, outbox: OutboxTx) {
    let entity_id = {
        let mut next = world.resource_mut::<NextEntityId>();
        let v = next.0;
        next.0 += 1;
        v
    };
    let (sx, sz) = SPAWN_XZ;
    // The column places grass at `surface_height` with air above, so stand the
    // player one block higher (their feet rest on top of the grass block).
    let sy = (crate::world::surface_height(sx as i32, sz as i32) + 1) as f64;
    let join = world.resource::<Config>().join_params();

    // The whole join sequence flows through the outbox. If it overflows mid-burst
    // (slow or hostile client) drop the connection rather than register a player
    // who never received the world.
    if !send_join_sequence(&outbox, entity_id, sx, sy, sz, &join) {
        warn!(%name, "outbox full during join sequence; dropping");
        let _ = outbox.try_send(Outbound::Close);
        return;
    }

    // Sync the world clock and current weather to the newcomer, mirroring
    // vanilla `PlayerList.sendLevelInfo`: a full SetTime (gameTime + the overworld
    // clock state, including its rate so frozen daylight is conveyed), then the
    // rain/thunder GameEvents if it is currently raining.
    send_world_state(world, &outbox);

    // Seed the loaded-chunk set to exactly the rounded view region the join just
    // streamed, centered on the spawn chunk (derived from `SPAWN_XZ`). Using the
    // same `in_view` predicate the streaming diff uses means the seeded set equals
    // what `send_join_sequence` streamed — no double-send, no gap — and the
    // streaming system (`stream_chunks`) sends only deltas as the player moves.
    let radius = join.view_distance;
    let spawn_center = ((sx.floor() as i32) >> 4, (sz.floor() as i32) >> 4);
    let mut loaded = std::collections::HashSet::new();
    for cx in (spawn_center.0 - radius - 1)..=(spawn_center.0 + radius + 1) {
        for cz in (spawn_center.1 - radius - 1)..=(spawn_center.1 + radius + 1) {
            if in_view(spawn_center, cx, cz, radius) {
                loaded.insert((cx, cz));
            }
        }
    }

    let tick = world.resource::<Tick>().0;
    info!(%name, entity_id, "joined");

    // Snapshot everyone already online before the newcomer is spawned. Their
    // spawn position/rotation comes from each player's `Tracking` base (the
    // last value broadcast), so the newcomer joins the shared delta stream in
    // sync — exactly what vanilla's entity tracker sends a new viewer.
    let existing: Vec<Existing> = {
        let mut q = world.query::<(&PlayerId, &Profile, &Tracking, &Meta, &Conn)>();
        q.iter(world)
            .map(|(pid, profile, t, meta, conn)| Existing {
                uuid: pid.0,
                name: profile.name.clone(),
                entity_id: profile.entity_id,
                base_x: t.base_x,
                base_y: t.base_y,
                base_z: t.base_z,
                yaw: t.yaw,
                pitch: t.pitch,
                head: t.head,
                sneaking: meta.sneaking,
                sprinting: meta.sprinting,
                outbox: conn.outbox.clone(),
            })
            .collect()
    };

    // Tell the newcomer about everyone already here: tab-list entries first
    // (the client resolves profiles from these), then spawn their entities.
    // The newcomer's own entry is included so they see themselves in the list.
    let mut newcomer_view: Vec<packets::PlayerEntry> = existing
        .iter()
        .map(|e| packets::PlayerEntry {
            uuid: e.uuid,
            name: e.name.clone(),
        })
        .collect();
    newcomer_view.push(packets::PlayerEntry {
        uuid: id,
        name: name.clone(),
    });
    send(&outbox, packets::player_info_update(&newcomer_view));
    for e in &existing {
        send(
            &outbox,
            packets::add_entity(
                e.entity_id,
                e.uuid,
                (e.base_x, e.base_y, e.base_z),
                e.yaw,
                e.pitch,
                e.head,
            ),
        );
        // AddEntity carries no metadata, so follow it with the current pose/flags
        // — otherwise an already-sneaking player would render standing.
        send(
            &outbox,
            packets::set_entity_data(e.entity_id, e.sneaking, e.sprinting),
        );
    }

    // Announce the newcomer to everyone already here: tab entry, then spawn.
    // Best-effort: if an existing player's outbox is momentarily full the send is
    // dropped and that client won't see the newcomer until a reconciling per-
    // player tracker exists (still pending — see ROADMAP). Acceptable while the
    // outbox is sized comfortably above a join burst.
    let newcomer_info = packets::player_info_update(&[packets::PlayerEntry {
        uuid: id,
        name: name.clone(),
    }]);
    let newcomer_spawn = packets::add_entity(entity_id, id, (sx, sy, sz), 0, 0, 0);
    // A fresh join is neither sneaking nor sprinting, but send the metadata for
    // parity with the existing-player path (and so the pose is explicitly reset).
    let newcomer_meta = packets::set_entity_data(entity_id, false, false);
    for e in &existing {
        let _ = e.outbox.try_send(Outbound::Packet(newcomer_info.clone()));
        let _ = e.outbox.try_send(Outbound::Packet(newcomer_spawn.clone()));
        let _ = e.outbox.try_send(Outbound::Packet(newcomer_meta.clone()));
    }

    // Replay non-player entities (dropped items, XP orbs, …) already in the world
    // to the newcomer, scoped to the columns they just loaded — the non-player
    // arm of the entity tracker (see `sim::entity`). Done before `outbox`/`loaded`
    // are moved into the player's components below.
    super::entity::spawn_existing_entities_for(world, &outbox, &loaded);

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
            Tracking {
                base_x: sx,
                base_y: sy,
                base_z: sz,
                yaw: 0,
                pitch: 0,
                head: 0,
                on_ground: false,
                teleport_delay: 0,
                tick_count: 0,
            },
            Meta::default(),
            LoadedChunks {
                center: spawn_center,
                loaded,
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
    // Center on the spawn chunk (derived from the spawn position) so the streamed
    // region tracks spawn automatically and matches the seeded `LoadedChunks`.
    let center = ((sx.floor() as i32) >> 4, (sz.floor() as i32) >> 4);
    ok &= send(outbox, packets::set_chunk_center(center.0, center.1));
    // Stream exactly the rounded view region (vanilla `ChunkTrackingView`) so the
    // client's "Loading terrain" wait is fully satisfied — no more, no less.
    let radius = join.view_distance;
    for cx in (center.0 - radius - 1)..=(center.0 + radius + 1) {
        for cz in (center.1 - radius - 1)..=(center.1 + radius + 1) {
            if in_view(center, cx, cz, radius) {
                ok &= send(outbox, packets::level_chunk(cx, cz));
            }
        }
    }
    ok &= send(
        outbox,
        packets::player_position(SPAWN_TELEPORT_ID, sx, sy, sz),
    );
    ok
}

/// Send a joining player the current world clock and weather (vanilla
/// `PlayerList.sendLevelInfo`). The clock is a full SetTime carrying the
/// overworld clock state (rate 0 if daylight is frozen); rain/thunder GameEvents
/// follow only when it is raining, matching the vanilla guard.
fn send_world_state(world: &World, outbox: &OutboxTx) {
    let rules = world.resource::<super::world_tick::GameRules>();
    let time = world.resource::<super::world_tick::WorldTime>();
    let weather = world.resource::<super::world_tick::Weather>();

    let clock = time.clock_update(rules.advance_time);
    send(outbox, packets::set_time(time.game_time, &[clock]));

    if weather.is_raining() {
        send(
            outbox,
            packets::game_event(packets::GAME_EVENT_START_RAINING, 0.0),
        );
        send(
            outbox,
            packets::game_event(packets::GAME_EVENT_RAIN_LEVEL_CHANGE, weather.rain_level),
        );
        send(
            outbox,
            packets::game_event(
                packets::GAME_EVENT_THUNDER_LEVEL_CHANGE,
                weather.thunder_level,
            ),
        );
    }
}

pub(super) fn on_left(world: &mut World, id: Uuid) {
    let entity = world.resource_mut::<PlayerIndex>().0.remove(&id);
    if let Some(e) = entity {
        let profile = world.get::<Profile>(e).map(|p| (p.name.clone(), p.entity_id));
        world.despawn(e);
        if let Some((name, entity_id)) = profile {
            // Drop the leaver from every remaining client's tab list and world.
            let info_remove = packets::player_info_remove(&[id]);
            let despawn = packets::remove_entities(&[entity_id]);
            let mut q = world.query::<&Conn>();
            for conn in q.iter(world) {
                let _ = conn.outbox.try_send(Outbound::Packet(info_remove.clone()));
                let _ = conn.outbox.try_send(Outbound::Packet(despawn.clone()));
            }
            info!(%name, "left");
        }
    }
}

/// Push a framed packet to an outbox, reporting whether it was accepted.
fn send(outbox: &OutboxTx, bytes: bytes::Bytes) -> bool {
    outbox.try_send(Outbound::Packet(bytes)).is_ok()
}
