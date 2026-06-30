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

/// How often (in ticks) an entity's position/rotation is broadcast, matching
/// vanilla's player `EntityType.updateInterval` of 2.
const UPDATE_INTERVAL: u32 = 2;

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
        Serverbound::Swing { hand } => {
            let Some(eid) = world.get::<Profile>(entity).map(|p| p.entity_id) else {
                return;
            };
            // InteractionHand: 0 = main hand, 1 = off hand. Anything else maps to
            // the main-arm swing (vanilla only ever sends the two).
            let action = if hand == 1 {
                packets::ANIMATE_SWING_OFF_HAND
            } else {
                packets::ANIMATE_SWING_MAIN_HAND
            };
            broadcast_to_others(world, entity, packets::animate(eid, action));
        }
        Serverbound::PlayerCommand { action } => {
            // 26.2 `Action` ordinals: 1 = START_SPRINTING, 2 = STOP_SPRINTING.
            // The others (sleeping, riding jump, open inventory, fall flying) have
            // no metadata effect here yet. Crouch is no longer reported via this
            // packet (see `Meta`), so only sprint toggles entity metadata.
            let sprinting = match action {
                1 => true,
                2 => false,
                _ => return,
            };
            let changed = world
                .get_mut::<Meta>(entity)
                .is_some_and(|mut m| std::mem::replace(&mut m.sprinting, sprinting) != sprinting);
            if changed {
                broadcast_meta(world, entity);
            }
        }
        Serverbound::PlayerAbilities { flags } => {
            // Only the flying bit (0x02) is meaningful serverbound; record it.
            let flying = flags & 0x02 != 0;
            if let Some(mut meta) = world.get_mut::<Meta>(entity) {
                meta.flying = flying;
            }
            debug!(flying, "player abilities");
        }
        Serverbound::PlayerInput { sneaking } => {
            // The SHIFT bit drives crouch in 26.2: toggle the sneaking flag and,
            // on a change, push the shared-flags/pose metadata to viewers (the
            // `set_entity_data` builder maps `sneaking` to the 0x02 flag bit and
            // the CROUCHING pose).
            let changed = world
                .get_mut::<Meta>(entity)
                .is_some_and(|mut m| std::mem::replace(&mut m.sneaking, sneaking) != sneaking);
            if changed {
                broadcast_meta(world, entity);
            }
        }
        // Update the player's selected hotbar slot. The `Inventory` component is
        // attached lazily on the first inventory packet so the join path stays
        // untouched.
        Serverbound::SetCarriedItem { slot } => {
            if (0..9).contains(&slot) {
                let mut ent = world.entity_mut(entity);
                if !ent.contains::<crate::inventory::Inventory>() {
                    ent.insert(crate::inventory::Inventory::new());
                }
                if let Some(mut inv) = ent.get_mut::<crate::inventory::Inventory>() {
                    inv.selected = slot as u8;
                }
            }
        }
        // Creative-mode slot set: write the stack into the addressed inventory
        // slot (server-side state only for now).
        Serverbound::SetCreativeSlot { slot, stack } => {
            let mut ent = world.entity_mut(entity);
            if !ent.contains::<crate::inventory::Inventory>() {
                ent.insert(crate::inventory::Inventory::new());
            }
            if let Some(mut inv) = ent.get_mut::<crate::inventory::Inventory>() {
                inv.set_slot(slot, stack);
            }
        }
    }
}

/// Fan a single framed packet out to every connection except `sender` — the
/// model `broadcast_movement` uses, lifted out for the one-shot action packets.
fn broadcast_to_others(world: &mut World, sender: Entity, bytes: bytes::Bytes) {
    let mut q = world.query::<(Entity, &Conn)>();
    for (e, conn) in q.iter(world) {
        if e == sender {
            continue;
        }
        let _ = conn.outbox.try_send(Outbound::Packet(bytes.clone()));
    }
}

/// Rebuild a player's entity-metadata packet from its current `Meta` and send it
/// to every other tracking connection.
fn broadcast_meta(world: &mut World, entity: Entity) {
    let Some(eid) = world.get::<Profile>(entity).map(|p| p.entity_id) else {
        return;
    };
    let Some((sneaking, sprinting)) = world.get::<Meta>(entity).map(|m| (m.sneaking, m.sprinting))
    else {
        return;
    };
    broadcast_to_others(world, entity, packets::set_entity_data(eid, sneaking, sprinting));
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

/// Broadcast each player's movement to everyone else, following vanilla's
/// `ServerEntity.sendChanges` for the player case: every `UPDATE_INTERVAL` ticks
/// pick the cheapest packet that conveys the change (a position and/or rotation
/// delta), fall back to an absolute resync when a delta won't do, and send head
/// yaw separately. Deltas are relative to each entity's last-sent `Tracking`
/// base, which is advanced only for the fields actually sent.
pub fn broadcast_movement(world: &mut World) {
    // Phase 1: decide each player's packets and advance their tracking state.
    let mut pending: Vec<(Entity, Vec<bytes::Bytes>)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &Profile, &Pos, &mut Tracking)>();
        for (entity, profile, pos, mut t) in q.iter_mut(world) {
            let mut packets = Vec::new();
            let eid = profile.entity_id;

            if t.tick_count % UPDATE_INTERVAL == 0 {
                t.teleport_delay += 1;

                let yaw_n = packets::pack_angle(pos.yaw);
                let pitch_n = packets::pack_angle(pos.pitch);
                let send_rotation = (yaw_n as i32 - t.yaw as i32).abs() >= 1
                    || (pitch_n as i32 - t.pitch as i32).abs() >= 1;

                let dx = pos.x - t.base_x;
                let dy = pos.y - t.base_y;
                let dz = pos.z - t.base_z;
                let position_changed = dx * dx + dy * dy + dz * dz >= 7.629_394_5e-6;
                // A forced position resend every 60 ticks corrects rounding drift
                // (vanilla `flag2 = flag1 || this.tickCount % 60 == 0`).
                let send_position = position_changed || t.tick_count % 60 == 0;

                let xa = packets::enc(pos.x) - packets::enc(t.base_x);
                let ya = packets::enc(pos.y) - packets::enc(t.base_y);
                let za = packets::enc(pos.z) - packets::enc(t.base_z);
                let delta_too_big = !(-32768..=32767).contains(&xa)
                    || !(-32768..=32767).contains(&ya)
                    || !(-32768..=32767).contains(&za);

                let mut sent_position = false;
                let mut sent_rotation = false;

                if delta_too_big || t.teleport_delay > 400 || t.on_ground != pos.on_ground {
                    // A relative delta won't do: resync absolutely.
                    t.on_ground = pos.on_ground;
                    t.teleport_delay = 0;
                    packets.push(packets::entity_position_sync(
                        eid, pos.x, pos.y, pos.z, pos.yaw, pos.pitch, pos.on_ground,
                    ));
                    sent_position = true;
                    sent_rotation = true;
                } else if !send_position || !send_rotation {
                    if send_position {
                        packets.push(packets::move_entity_pos(
                            eid,
                            xa as i16,
                            ya as i16,
                            za as i16,
                            pos.on_ground,
                        ));
                        sent_position = true;
                    } else if send_rotation {
                        packets.push(packets::move_entity_rot(
                            eid,
                            yaw_n,
                            pitch_n,
                            pos.on_ground,
                        ));
                        sent_rotation = true;
                    }
                } else {
                    packets.push(packets::move_entity_pos_rot(
                        eid,
                        xa as i16,
                        ya as i16,
                        za as i16,
                        yaw_n,
                        pitch_n,
                        pos.on_ground,
                    ));
                    sent_position = true;
                    sent_rotation = true;
                }

                if sent_position {
                    t.base_x = pos.x;
                    t.base_y = pos.y;
                    t.base_z = pos.z;
                }
                if sent_rotation {
                    t.yaw = yaw_n;
                    t.pitch = pitch_n;
                }

                // Head yaw is independent of the body yaw in general, but we
                // don't model a separate body yaw, so it reuses the packed look
                // yaw the move packets already carry.
                let head_n = yaw_n;
                if (head_n as i32 - t.head as i32).abs() >= 1 {
                    packets.push(packets::rotate_head(eid, head_n));
                    t.head = head_n;
                }
            }

            t.tick_count = t.tick_count.wrapping_add(1);

            if !packets.is_empty() {
                pending.push((entity, packets));
            }
        }
    }

    if pending.is_empty() {
        return;
    }

    // Phase 2: fan each player's packets out to every other connection.
    let conns: Vec<(Entity, OutboxTx)> = {
        let mut q = world.query::<(Entity, &Conn)>();
        q.iter(world).map(|(e, c)| (e, c.outbox.clone())).collect()
    };
    for (sender, packets) in pending {
        for (entity, outbox) in &conns {
            if *entity == sender {
                continue;
            }
            for pkt in &packets {
                let _ = outbox.try_send(Outbound::Packet(pkt.clone()));
            }
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

/// Push a framed packet to an outbox, reporting whether it was accepted.
fn send(outbox: &OutboxTx, bytes: bytes::Bytes) -> bool {
    outbox.try_send(Outbound::Packet(bytes)).is_ok()
}
