//! Serverbound Play packet handling: apply each decoded `Serverbound` message to
//! the world (movement intent, chat, commands, block dig/place, inventory, …) and
//! fan the resulting changes out to the affected connections. Driven by
//! `systems::drain_ingress`.

use bevy_ecs::prelude::*;
use tracing::{debug, info};
use uuid::Uuid;

use super::bridge::{Outbound, Serverbound};
use super::commands;
use super::components::*;
use super::packets;

pub(super) fn on_packet(world: &mut World, id: Uuid, packet: Serverbound) {
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
                inventory_mut(world, entity).selected = slot as u8;
            }
        }
        // Creative-mode slot set: write the stack into the addressed inventory
        // slot (server-side state only for now).
        Serverbound::SetCreativeSlot { slot, stack } => {
            inventory_mut(world, entity).set_slot(slot, stack);
        }
        // Block dig. The server advertises SURVIVAL (`GAME_TYPE_SURVIVAL`), so we
        // mirror `ServerPlayerGameMode.handleBlockBreakAction` for a non-instabuild
        // player: the block is destroyed on STOP_DESTROY_BLOCK (the completion
        // signal the client sends when the dig finishes), not on START. ABORT and
        // the non-dig actions only need their sequence acknowledged.
        //
        // KNOWN GAP: vanilla survival *also* destroys instantly on START when the
        // block's destroy progress reaches 1 in a single tick (0-hardness blocks
        // like flowers/torches). We have no block-hardness model yet, so every
        // block waits for STOP. (A creative client would send only START and break
        // instantly; we don't advertise creative or track a per-player game mode,
        // so that path isn't modelled here.)
        Serverbound::PlayerAction {
            action,
            x,
            y,
            z,
            face: _,
            sequence,
        } => {
            // 2 = STOP_DESTROY_BLOCK (dig completed).
            if action == 2 {
                let prev = crate::world::set_block(x, y, z, crate::world::AIR_STATE);
                if prev != crate::world::AIR_STATE {
                    broadcast_block_update(
                        world,
                        x,
                        z,
                        packets::block_update(x, y, z, crate::world::AIR_STATE),
                    );
                }
            }
            ack_block_change(world, entity, sequence);
        }
        // Block place. Resolve the held hotbar item to a block-state, place it on
        // the face the player clicked (target + face step), broadcast the change,
        // and acknowledge the sequence so the client keeps its prediction.
        Serverbound::UseItemOn {
            hand: _,
            x,
            y,
            z,
            face,
            cursor_x: _,
            cursor_y: _,
            cursor_z: _,
            inside: _,
            sequence,
        } => {
            if let Some(state) = held_block_state(world, entity) {
                let (dx, dy, dz) = face_step(face);
                let (px, py, pz) = (x + dx, y + dy, z + dz);
                // Only place into air, mirroring vanilla's replaceable check
                // (loosely — air is the one replaceable state we model).
                if crate::world::block_state_at(px, py, pz) == crate::world::AIR_STATE {
                    crate::world::set_block(px, py, pz, state);
                    // Simplified placement: the held stack is NOT decremented and
                    // there is no reach/cursor validation — this is infinite-blocks
                    // demo placement, not survival inventory logic.
                    broadcast_block_update(world, px, pz, packets::block_update(px, py, pz, state));
                }
            }
            ack_block_change(world, entity, sequence);
        }
    }
}

/// The block-state the player would place: their selected hotbar item mapped
/// through the item→block table. `None` if no inventory, an empty slot, or a
/// non-placeable item.
fn held_block_state(world: &World, entity: Entity) -> Option<u32> {
    let inv = world.get::<crate::inventory::Inventory>(entity)?;
    let slot = crate::inventory::HOTBAR_START + inv.selected as usize;
    let item_id = inv.slots[slot]?.id;
    crate::world::block_state_for_item(item_id)
}

/// The unit step of a `Direction` 3D-data value (`Direction.java`): 0 DOWN,
/// 1 UP, 2 NORTH, 3 SOUTH, 4 WEST, 5 EAST. Unknown values don't move.
fn face_step(face: i32) -> (i32, i32, i32) {
    match face {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        5 => (1, 0, 0),
        _ => (0, 0, 0),
    }
}

/// Acknowledge a block-change sequence to the acting player so the client
/// confirms (rather than rolls back) its predicted edit.
fn ack_block_change(world: &mut World, entity: Entity, sequence: i32) {
    if let Some(conn) = world.get::<Conn>(entity) {
        let _ = conn
            .outbox
            .try_send(Outbound::Packet(packets::block_changed_ack(sequence)));
    }
}

/// Fan a block-change packet out to exactly the players tracking the affected
/// column `(bx>>4, bz>>4)`, mirroring vanilla — `ServerLevel.blockUpdated` routes
/// `ClientboundBlockUpdatePacket` through the chunk's tracking players, not every
/// connection. The actor is included (they observe their own edit).
fn broadcast_block_update(world: &mut World, bx: i32, bz: i32, bytes: bytes::Bytes) {
    let cc = (bx >> 4, bz >> 4);
    let mut q = world.query::<(&Conn, &LoadedChunks)>();
    for (conn, loaded) in q.iter(world) {
        if loaded.loaded.contains(&cc) {
            let _ = conn.outbox.try_send(Outbound::Packet(bytes.clone()));
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

/// Borrow a player's `Inventory`, attaching a fresh empty one on first use. The
/// component is added lazily (rather than at join) so the inventory module stays
/// out of the join path; both serverbound inventory packets funnel through here.
fn inventory_mut(world: &mut World, entity: Entity) -> Mut<'_, crate::inventory::Inventory> {
    if world.get::<crate::inventory::Inventory>(entity).is_none() {
        world
            .entity_mut(entity)
            .insert(crate::inventory::Inventory::new());
    }
    world
        .get_mut::<crate::inventory::Inventory>(entity)
        .expect("inventory just inserted")
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
