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
    // Choose a solid, dry spawn column from the generator rather than the fixed
    // origin (which may be ocean under the biome/height field).
    let spawn = crate::world::spawn_column();
    let (mut sx, mut sz) = (spawn.0 as f64, spawn.1 as f64);
    // The column's surface block sits at `surface_height` with air above, so stand
    // the player one block higher (their feet rest on top of the surface block).
    let mut sy = (crate::world::surface_height(sx as i32, sz as i32) + 1) as f64;
    let mut syaw = 0.0f32;
    let mut spitch = 0.0f32;
    let mut son_ground = false;
    // Fresh-spawn inventory defaults (empty, first hotbar slot selected), replaced
    // below if the player has saved data.
    let mut inv_slots = [None; crate::inventory::PLAYER_INVENTORY_SLOTS];
    let mut inv_selected: u8 = 0;
    // Fresh-spawn survival defaults (full health, full food bar), replaced below if
    // the player has saved data.
    let mut health = super::survival::Health::new(super::survival::MAX_HEALTH);
    let mut food = super::survival::FoodData::default();
    // Restore a returning player's saved position/orientation/inventory, if any. A
    // read error is logged and the fresh-spawn defaults above are kept.
    if let Some(dir) = crate::world::storage::player_data_dir() {
        match crate::world::storage::PlayerData::load(&dir, id) {
            Ok(Some(pd)) => {
                sx = pd.x;
                sy = pd.y;
                sz = pd.z;
                syaw = pd.yaw;
                spitch = pd.pitch;
                son_ground = pd.on_ground;
                inv_slots = pd.inventory;
                inv_selected = pd.selected_slot.clamp(0, 8) as u8;
                health = super::survival::Health::new(pd.health);
                food = super::survival::FoodData {
                    food_level: pd.food_level,
                    saturation_level: pd.food_saturation,
                    exhaustion_level: pd.food_exhaustion,
                    tick_timer: pd.food_tick_timer,
                };
            }
            Ok(None) => {}
            Err(e) => warn!(%name, error = %e, "failed to read player data; spawning fresh"),
        }
    }
    // The player's game mode is attached at join (seeded from the server-default
    // `gamemode`) so every system can rely on it being present — matching vanilla,
    // where a `ServerPlayer` always has a `GameType`. Per-player persistence /
    // `/gamemode` is a separate follow-up; for now every player gets the default.
    let game_mode = GameMode::from_id(world.resource::<Config>().0.properties.gamemode());
    // Seed the player's inventory component from the saved (or empty) snapshot so
    // it, too, exists from the first tick — no lazy attach.
    let mut inventory = crate::inventory::Inventory::new();
    inventory.slots = inv_slots;
    inventory.selected = inv_selected;
    let join = world.resource::<Config>().join_params();

    // The whole join sequence flows through the outbox. If it overflows mid-burst
    // (slow or hostile client) drop the connection rather than register a player
    // who never received the world.
    if !send_join_sequence(&outbox, entity_id, sx, sy, sz, syaw, spitch, &join) {
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
    // streamed, centered on the spawn chunk (derived from the spawn column). Using the
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
    let packed_yaw = packets::pack_angle(syaw);
    let packed_pitch = packets::pack_angle(spitch);
    let newcomer_spawn = packets::add_entity(
        entity_id,
        id,
        (sx, sy, sz),
        packed_yaw,
        packed_pitch,
        packed_yaw,
    );
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

    // Sync the (loaded or fresh) inventory to the newcomer, mirroring vanilla
    // `PlayerList.sendAllPlayerInfo`: a full `ContainerSetContent` for the player
    // menu (window 0) plus the selected-hotbar `SetHeldSlot`. Without this the
    // server holds a restored inventory the client never hears about, so a
    // returning player sees an empty inventory until an interaction forces a
    // resync. (Vela has no per-tick `broadcastChanges`; this is the one-shot
    // equivalent.)
    let inv_state_id = inventory.next_state_id();
    send(
        &outbox,
        crate::inventory::container_set_content(
            0,
            inv_state_id,
            &inventory.slots,
            inventory.carried.as_ref(),
        ),
    );
    send(&outbox, crate::inventory::set_held_slot(inventory.selected as i32));

    let entity = world
        .spawn((
            PlayerId(id),
            Profile { name, entity_id },
            Pos {
                x: sx,
                y: sy,
                z: sz,
                yaw: syaw,
                pitch: spitch,
                on_ground: son_ground,
            },
            Tracking {
                base_x: sx,
                base_y: sy,
                base_z: sz,
                yaw: packed_yaw,
                pitch: packed_pitch,
                head: packed_yaw,
                on_ground: son_ground,
                teleport_delay: 0,
                tick_count: 0,
            },
            Meta::default(),
            super::chat::ChatState::default(),
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
            inventory,
            game_mode,
            health,
            food,
        ))
        .id();
    world.resource_mut::<PlayerIndex>().0.insert(id, entity);

    // Sync the player's HUD (health/food/saturation) now that the entity exists,
    // mirroring the initial `SetHealth` vanilla sends in the join sequence.
    super::survival::send_initial_health(world, entity);
}

/// Build and push the join sequence. Ordering matters: the GameEvent puts the
/// client in its "waiting for chunks" state, the chunks satisfy that wait, then
/// the teleport settles the player. Returns `false` if any send fails.
#[allow(clippy::too_many_arguments)]
fn send_join_sequence(
    outbox: &OutboxTx,
    entity_id: i32,
    sx: f64,
    sy: f64,
    sz: f64,
    syaw: f32,
    spitch: f32,
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
        packets::player_position(SPAWN_TELEPORT_ID, sx, sy, sz, syaw, spitch),
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
        save_player_data(world, e, id);
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

/// Persist every currently-online player's data, mirroring vanilla
/// `PlayerList.saveAll`. Called from the periodic autosave and the shutdown save
/// so a server stopped (Ctrl+C / `/stop`) with players still connected does not
/// lose their inventory/position — `on_left` only runs on an individual
/// disconnect, which never happens for a player online at shutdown.
pub(super) fn save_all_players(world: &mut World) {
    if crate::world::storage::player_data_dir().is_none() {
        return;
    }
    let players: Vec<(Entity, Uuid)> = {
        let mut q = world.query::<(Entity, &PlayerId)>();
        q.iter(world).map(|(e, pid)| (e, pid.0)).collect()
    };
    for (entity, id) in players {
        save_player_data(world, entity, id);
    }
}

/// Persist a leaving player's position, orientation, and inventory to
/// `playerdata/<uuid>.dat`. A no-op when persistence is disabled. The inventory
/// (and selected hotbar slot) come from the player's `Inventory` component, which
/// is now attached at join, so both are always present for a real player.
fn save_player_data(world: &World, entity: Entity, id: Uuid) {
    let Some(dir) = crate::world::storage::player_data_dir() else {
        return;
    };
    let Some(pos) = world.get::<Pos>(entity) else {
        return;
    };
    let (selected_slot, inventory) = world
        .get::<crate::inventory::Inventory>(entity)
        .map(|inv| (inv.selected as i32, inv.slots))
        .unwrap_or((0, [None; crate::inventory::PLAYER_INVENTORY_SLOTS]));
    // Health/food fall back to a fresh player's defaults if the components are
    // somehow absent (they're attached at join for every real player).
    let health = world
        .get::<super::survival::Health>(entity)
        .map(|h| h.current)
        .unwrap_or(super::survival::MAX_HEALTH);
    let food = world.get::<super::survival::FoodData>(entity).copied().unwrap_or_default();
    let data = crate::world::storage::PlayerData {
        x: pos.x,
        y: pos.y,
        z: pos.z,
        yaw: pos.yaw,
        pitch: pos.pitch,
        on_ground: pos.on_ground,
        health,
        food_level: food.food_level,
        food_saturation: food.saturation_level,
        food_exhaustion: food.exhaustion_level,
        food_tick_timer: food.tick_timer,
        selected_slot,
        inventory,
    };
    if let Err(e) = data.save(&dir, id) {
        warn!(error = %e, "failed to save player data");
    }
}

/// Push a framed packet to an outbox, reporting whether it was accepted.
fn send(outbox: &OutboxTx, bytes: bytes::Bytes) -> bool {
    outbox.try_send(Outbound::Packet(bytes)).is_ok()
}
