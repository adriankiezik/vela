//! Serverbound Play packet handling: apply each decoded `Serverbound` message to
//! the world (movement intent, chat, commands, block dig/place, inventory, …) and
//! fan the resulting changes out to the affected connections. Driven by
//! `systems::drain_ingress`.

use bevy_ecs::prelude::*;
use tracing::{debug, info};
use uuid::Uuid;

use super::bridge::{Outbound, Serverbound};
use super::chat::{self, ChatSession, ChatState};
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
            // Snapshot the pre-move position so fall distance (and sprint
            // exhaustion) can be derived from the delta the client just reported.
            let before = world.get::<Pos>(entity).map(|p| (p.x, p.y, p.z));
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
            // Fall-distance tracking / fall damage / sprint exhaustion (only when
            // the player carries survival state — a real player always does).
            if let (Some(before), Some(after)) =
                (before, world.get::<Pos>(entity).map(|p| (p.x, p.y, p.z)))
            {
                let sprinting = world.get::<Meta>(entity).is_some_and(|m| m.sprinting);
                super::survival::handle_move(world, entity, before, after, on_ground, sprinting);
            }
        }
        Serverbound::Chat {
            message,
            timestamp,
            salt,
            signature,
        } => on_chat(world, entity, message, timestamp, salt, signature),
        Serverbound::ChatSessionUpdate {
            session_id,
            expires_at,
            public_key,
            key_signature,
        } => {
            // Store the player's chat session (head of its signing chain). We
            // keep the key/signature verbatim; verifying it against the yggdrasil
            // service key is deferred (see `sim::chat`).
            if let Some(mut state) = world.get_mut::<ChatState>(entity) {
                state.session = Some(ChatSession {
                    session_id,
                    expires_at,
                    public_key,
                    key_signature,
                });
                debug!(%session_id, "chat session updated");
            }
        }
        Serverbound::CommandSuggestion { id, command } => {
            let (start, length, matches) = commands::suggest(world, &command);
            send_to_self(world, entity, chat::command_suggestions(id, start, length, &matches));
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
        // The client finished loading the level around the player. Clear the load
        // gate (vanilla `markClientLoaded`), which ends the invulnerability window
        // opened at join/respawn — the player's own chunk is now on the client, so
        // it will settle on the ground rather than free-fall through the void.
        Serverbound::PlayerLoaded => {
            if let Some(mut gate) = world.get_mut::<ClientLoaded>(entity) {
                gate.loaded = true;
            }
        }
        // The client resent its settings mid-session. Vanilla `updateOptions`
        // copies `viewDistance` into `requestedViewDistance`; when it changes, the
        // effective radius (`getPlayerViewDistance`) changes, so re-diff the
        // player's tracked chunks against the new square. Other options are dropped.
        Serverbound::ClientInformation { view_distance } => {
            let changed = world
                .get_mut::<RequestedViewDistance>(entity)
                .is_some_and(|mut r| std::mem::replace(&mut r.0, view_distance) != view_distance);
            if changed {
                debug!(view_distance, "client view distance changed");
                super::chunking::apply_view_distance_change(world, entity);
            }
        }
        // The client acknowledged a chunk batch and reports its sustainable rate.
        // Feed the per-player `ChunkSender` throttle so the next batch is released
        // (`PlayerChunkSender.onChunkBatchReceivedByClient`).
        Serverbound::ChunkBatchReceived {
            desired_chunks_per_tick,
        } => {
            if let Some(mut sender) = world.get_mut::<ChunkSender>(entity) {
                sender.on_ack(desired_chunks_per_tick);
            }
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
        Serverbound::Attack { entity_id } => on_attack(world, entity, entity_id),
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
        // A click in an open menu (player inventory id 0, or an open chest).
        Serverbound::ContainerClick {
            container_id,
            state_id,
            slot,
            button,
            mode,
        } => on_container_click(world, entity, container_id, state_id, slot, button, mode),
        // The player closed the open screen.
        Serverbound::ContainerClose { container_id } => {
            on_container_close(world, entity, container_id)
        }
        // Death-screen "Respawn" button (and stats/gamerule requests we ignore).
        // 0 = PERFORM_RESPAWN.
        Serverbound::ClientCommand { action } => {
            if action == 0 {
                super::survival::respawn_player(world, entity);
            }
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
            // 2 = STOP_DESTROY_BLOCK (dig completed). Gate on the player's loaded
            // view, mirroring vanilla: a serverbound dig lands on `ServerLevel`
            // for a chunk loaded server-side, which for a legitimate client is
            // exactly a column in its `ChunkTrackingView`. Vela's equivalent is
            // the player's `LoadedChunks` set. Inside that set the target may be
            // reference-counted yet not *yet* resident (columns warm through the
            // background prefetch pool), so we take the generating `set_block`
            // path — a rare, player-driven, single-column build is acceptable on
            // the tick thread, and dropping the dig instead would desync the
            // client's predicted break. Out-of-view (e.g. cheated) digs are
            // ignored, so this never generates a far chunk on the tick thread.
            if action == 2 && column_loaded(world, entity, x, z) {
                let prev = crate::world::set_block(x, y, z, crate::world::AIR_STATE);
                if prev != crate::world::AIR_STATE {
                    broadcast_block_update(
                        world,
                        x,
                        z,
                        packets::block_update(x, y, z, crate::world::AIR_STATE),
                    );
                    // Spawn the block's drops as item entities, mirroring
                    // `Block.playerDestroy` → `dropResources` → `popResource`.
                    // Only survival drops: creative destroys without drops, and
                    // adventure/spectator either can't break or don't drop (we gate
                    // strictly on Survival for now — see task scope). The pickup
                    // system will later consume these `ItemDrop` entities.
                    if player_game_mode(world, entity) == GameMode::Survival {
                        for stack in crate::world::block_drop::drops_for(prev) {
                            let pos = drop_position(x, y, z);
                            super::entity::spawn_item_entity(world, pos, stack);
                        }
                    }
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
                // (loosely — air is the one replaceable state we model). Gated on
                // the player's loaded view like the dig above: inside it the
                // target column may be referenced but not yet warmed, so use the
                // generating read/write (a one-off single-column build is fine for
                // a player-driven place). An out-of-view target is ignored, so
                // this never generates a far chunk on the tick thread.
                if column_loaded(world, entity, px, pz)
                    && crate::world::block_state_at(px, py, pz) == crate::world::AIR_STATE
                {
                    crate::world::set_block(px, py, pz, state);
                    broadcast_block_update(world, px, pz, packets::block_update(px, py, pz, state));
                    // Consume one from the held stack in survival, matching vanilla
                    // `BlockItem.place` → `ItemStack.shrink(1)` (creative places for
                    // free). The client predicts this decrement, so we resync the
                    // authoritative inventory to confirm it — without the consume,
                    // the client's predicted −1 and the server's untouched stack
                    // desync, surfacing as a doubled count once a later
                    // server-driven resync (e.g. breaking the block) lands.
                    if player_game_mode(world, entity) == GameMode::Survival {
                        consume_held_item(world, entity);
                    }
                }
            }
            ack_block_change(world, entity, sequence);
        }
    }
}

/// Whether the player is tracking the column containing world `(x, z)` — i.e.
/// the block sits in a chunk loaded server-side for this player. This is Vela's
/// stand-in for vanilla gating a serverbound block edit on the target chunk being
/// loaded within the player's ticket/view range: a legitimate dig or place only
/// ever targets a column in the player's `ChunkTrackingView`, which here is the
/// `LoadedChunks` set. It lets the edit handlers take the generating world path
/// (warming a referenced-but-not-yet-resident column on demand) while still
/// refusing to generate an arbitrary far column for an out-of-view edit.
fn column_loaded(world: &World, entity: Entity, x: i32, z: i32) -> bool {
    world
        .get::<LoadedChunks>(entity)
        .is_some_and(|lc| lc.loaded.contains(&(x >> 4, z >> 4)))
}

/// The block-state the player would place: their selected hotbar item mapped
/// through the item→block table. `None` if no inventory, an empty slot, or a
/// non-placeable item.
fn held_block_state(world: &World, entity: Entity) -> Option<crate::ids::BlockState> {
    let inv = world.get::<crate::inventory::Inventory>(entity)?;
    let slot = crate::inventory::HOTBAR_START + inv.selected as usize;
    let item_id = inv.slots[slot]?.id;
    crate::world::block_state_for_item(item_id)
}

/// Consume one item from the player's selected hotbar slot after a successful
/// placement (vanilla `BlockItem.place` → `ItemStack.shrink(1)`), clearing the
/// slot when it empties, then resync the authoritative inventory to the client so
/// its predicted decrement is confirmed rather than doubled.
fn consume_held_item(world: &mut World, entity: Entity) {
    let content = {
        let mut inv = inventory_mut(world, entity);
        let slot = crate::inventory::HOTBAR_START + inv.selected as usize;
        match &mut inv.slots[slot] {
            Some(stack) => {
                stack.shrink(1);
                if stack.is_empty() {
                    inv.slots[slot] = None;
                }
            }
            None => return, // nothing held (shouldn't happen — caller checked)
        }
        let state_id = inv.next_state_id();
        crate::inventory::container_set_content(0, state_id, &inv.slots, inv.carried.as_ref())
    };
    send_to_self(world, entity, content);
}

/// The breaking player's game mode, attached lazily (like `Inventory`) so the
/// join path stays untouched. First use seeds it from the server-default
/// `gamemode` in `server.properties` (`GameType.byId`); once persisted per-player
/// modes / `/gamemode` exist, this component is where they live.
fn player_game_mode(world: &mut World, entity: Entity) -> GameMode {
    if let Some(gm) = world.get::<GameMode>(entity) {
        return *gm;
    }
    let default = GameMode::from_id(world.resource::<Config>().0.properties.gamemode());
    world.entity_mut(entity).insert(default);
    default
}

/// The spawn position for a dropped item at block `(x, y, z)`, matching
/// `Block.popResource` (MC 26.2): block center `+0.5` on each axis, jittered by
/// `Mth.nextDouble(random, -0.25, 0.25)` (= `random.nextDouble() * 0.5 - 0.25`),
/// with the Y additionally lowered by half the item entity's height (its bbox is
/// `EntityType.ITEM` = 0.25 tall, so `-0.125`). We reuse the project's `rand`
/// (the same crate `sim::entity` uses for entity UUIDs); vanilla draws from the
/// level RNG, which is likewise non-deterministic here. Velocity is not applied:
/// item physics/motion is not modelled yet (`sim::entity` spawns items at rest),
/// so the small `(±0.1, 0.2, ±0.1)` `setDeltaMovement` kick is intentionally
/// dropped — noted for when item ticking lands.
fn drop_position(x: i32, y: i32, z: i32) -> (f64, f64, f64) {
    /// Half of `EntityType.ITEM`'s 0.25-block height (`popResource`'s `halfHeight`).
    const HALF_ITEM_HEIGHT: f64 = 0.125;
    let jitter = || rand::random::<f64>() * 0.5 - 0.25;
    (
        x as f64 + 0.5 + jitter(),
        y as f64 + 0.5 + jitter() - HALF_ITEM_HEIGHT,
        z as f64 + 0.5 + jitter(),
    )
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

/// Resolve a `ContainerClick` against the player's open menu and re-sync the
/// authoritative result to that player.
///
/// Mirrors `ServerGamePacketListenerImpl.handleContainerClick`: the click only
/// applies when its `containerId` matches the open menu. We then run the ported
/// `Menu` click state machine over a snapshot, write the result back, and send a
/// full `ContainerSetContent` (always-correct superset of vanilla's incremental
/// `broadcastChanges` — vanilla itself falls back to a full sync on a state-id
/// mismatch). The client's predicted slots are ignored; the server is the source
/// of truth.
fn on_container_click(
    world: &mut World,
    entity: Entity,
    container_id: i32,
    _state_id: i32,
    slot: i16,
    button: i8,
    mode: i32,
) {
    use crate::inventory::{ClickType, Menu, OpenContainer};

    let click = ClickType::from_wire(mode);
    // Snapshot the inventory (and drag state) up front so we can build the menu
    // without holding a borrow across the menu run.
    let (slots, carried, drag_status, drag_type, drag_slots) = {
        let inv = inventory_mut(world, entity);
        (
            inv.slots,
            inv.carried,
            inv.drag_status,
            inv.drag_type,
            inv.drag_slots.clone(),
        )
    };

    if container_id == 0 {
        // The always-open player inventory menu.
        let mut menu = Menu::player(&slots, carried);
        menu.set_drag(drag_status, drag_type, drag_slots);
        if !menu.is_valid_slot_index(slot as i32) {
            return;
        }
        menu.clicked(slot as i32, button as i32, click, false);

        let new_slots = menu.player_slots();
        let new_carried = menu.carried();
        let (ds, dt, dl) = menu.drag();
        let content = menu.content();

        let state_id = {
            let mut inv = inventory_mut(world, entity);
            inv.slots = new_slots;
            inv.carried = new_carried;
            inv.drag_status = ds;
            inv.drag_type = dt;
            inv.drag_slots = dl;
            inv.next_state_id()
        };
        send_to_self(
            world,
            entity,
            crate::inventory::container_set_content(0, state_id, &content, new_carried.as_ref()),
        );
        return;
    }

    // An open chest (or other non-zero container).
    let Some((rows, chest_items)) = world
        .get::<OpenContainer>(entity)
        .filter(|oc| oc.container_id == container_id)
        .map(|oc| (oc.rows, oc.items.clone()))
    else {
        return; // no such menu open
    };

    let mut menu = Menu::chest(rows, &chest_items, &slots, carried);
    menu.set_drag(drag_status, drag_type, drag_slots);
    if !menu.is_valid_slot_index(slot as i32) {
        return;
    }
    menu.clicked(slot as i32, button as i32, click, false);

    let new_player = menu.player_slots();
    let new_chest = menu.chest_slots();
    let new_carried = menu.carried();
    let (ds, dt, dl) = menu.drag();
    let content = menu.content();

    let state_id = {
        let mut inv = inventory_mut(world, entity);
        inv.slots = new_player;
        inv.carried = new_carried;
        inv.drag_status = ds;
        inv.drag_type = dt;
        inv.drag_slots = dl;
        inv.next_state_id()
    };
    if let Some(mut oc) = world.get_mut::<OpenContainer>(entity) {
        oc.items = new_chest;
    }
    send_to_self(
        world,
        entity,
        crate::inventory::container_set_content(container_id, state_id, &content, new_carried.as_ref()),
    );
}

/// Handle a menu close: mirror `AbstractContainerMenu.removed` for the cursor
/// (place it back into the inventory, or discard if there's no room — we have no
/// item-drop entities), drop the open-container state, and re-sync the player
/// inventory.
fn on_container_close(world: &mut World, entity: Entity, _container_id: i32) {
    let state_id = {
        let mut inv = inventory_mut(world, entity);
        if let Some(carried) = inv.carried.take() {
            place_into_inventory(&mut inv.slots, carried);
        }
        inv.drag_status = 0;
        inv.drag_type = -1;
        inv.drag_slots.clear();
        inv.next_state_id()
    };
    world.entity_mut(entity).remove::<crate::inventory::OpenContainer>();

    let slots = inventory_mut(world, entity).slots;
    let items: Vec<_> = slots.to_vec();
    send_to_self(
        world,
        entity,
        crate::inventory::container_set_content(0, state_id, &items, None),
    );
}

/// Place a stack into the player inventory (menu-ordered): merge into matching
/// hotbar (36..45) then main (9..36) slots, then the first empty one. Leftovers
/// are discarded (no item-drop entities yet). A minimal `Inventory.add`.
fn place_into_inventory(
    slots: &mut [Option<crate::inventory::ItemStack>; crate::inventory::PLAYER_INVENTORY_SLOTS],
    mut stack: crate::inventory::ItemStack,
) {
    let regions = [(36usize, 45usize), (9, 36)];
    // Merge pass: top up matching stacks.
    for &(s, e) in &regions {
        for slot in slots[s..e].iter_mut() {
            if stack.count <= 0 {
                return;
            }
            if let Some(existing) = slot {
                if existing.id == stack.id {
                    let max = 64.min(stack.max_stack_size());
                    let room = max - existing.count;
                    if room > 0 {
                        let moved = room.min(stack.count);
                        existing.count += moved;
                        stack.count -= moved;
                    }
                }
            }
        }
    }
    // Placement pass: drop the remainder into the first empty slot.
    for &(s, e) in &regions {
        for slot in slots[s..e].iter_mut() {
            if stack.count <= 0 {
                return;
            }
            if slot.is_none() {
                *slot = Some(stack);
                stack.count = 0;
            }
        }
    }
}

/// Send a framed packet to a single player's own connection.
fn send_to_self(world: &mut World, entity: Entity, bytes: bytes::Bytes) {
    if let Some(conn) = world.get::<Conn>(entity) {
        let _ = conn.outbox.try_send(Outbound::Packet(bytes));
    }
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

/// Run a `/command` for `sender`. Like vanilla's `sendSuccess(..., false)`, a
/// reply goes only to the player who issued it; dispatch and the per-command
/// handlers live in `commands`. A handler that fanned out its own output (chat
/// broadcast, private message) returns `None` and no reply is sent here.
fn on_command(world: &mut World, sender: Entity, line: &str) {
    let Some(name) = world.get::<Profile>(sender).map(|p| p.name.clone()) else {
        return;
    };
    info!(%name, command = %line, "command");

    if let Some(reply) = commands::run(world, sender, line) {
        let bytes = packets::system_chat_component(&reply);
        send_to_self(world, sender, bytes);
    }
}

/// Bare-hand attack damage — `Attributes.ATTACK_DAMAGE`'s base value for a player
/// (1.0). Held-weapon bonuses come from item attribute modifiers, which Vela has
/// no model for yet (see the attribute note in `sim::mob`), so every hit is the
/// fist's 1.0 for now — enough to kill a chicken in 4 hits, a pig/cow in 10, as
/// bare-handed vanilla does.
const PLAYER_ATTACK_DAMAGE: f32 = 1.0;

/// Max squared distance from attacker to target for a melee hit to register. A
/// lenient server-side sanity gate around vanilla's 3.0-block reach
/// (`Player.isWithinAttackRange`), widened here to absorb the target's bounding
/// box, the attacker's eye height, and movement latency — the client already
/// raycast the target before sending, so this only rejects wildly out-of-range
/// (e.g. cheated) attacks rather than reproducing vanilla's exact AABB test.
const ATTACK_REACH_SQ: f64 = 6.0 * 6.0;

/// Handle a `ServerboundAttackPacket`: the player left-clicked an entity. Resolve
/// the target by its network id, gate on game mode and reach, then apply melee
/// damage — the player→mob half of the combat seam, mirroring
/// `ServerGamePacketListenerImpl.handleAttack` → `Player.attack`. Attacks on
/// non-mob entities (other players, dropped items, XP orbs) are ignored for now:
/// PvP routes through `survival::hurt` and item entities aren't attackable — both
/// separate milestones.
fn on_attack(world: &mut World, attacker: Entity, target_net_id: i32) {
    // Spectators can't attack (handleAttack's `!player.isSpectator()` gate). An
    // unattached game mode means the server default (survival), which may attack.
    if world.get::<GameMode>(attacker) == Some(&GameMode::Spectator) {
        return;
    }
    // Resolve the target to a live, damageable mob by its network id.
    let Some(target) = find_mob_by_net_id(world, target_net_id) else {
        return; // unknown id, or the entity isn't a mob
    };
    // Reach check: reject a hit thrown from implausibly far away.
    let attacker_pos = world.get::<Pos>(attacker).map(|p| (p.x, p.y, p.z));
    let within_reach = match (attacker_pos, world.get::<Pos>(target).map(|p| (p.x, p.y, p.z))) {
        (Some(a), Some(t)) => {
            let (dx, dy, dz) = (a.0 - t.0, a.1 - t.1, a.2 - t.2);
            dx * dx + dy * dy + dz * dz <= ATTACK_REACH_SQ
        }
        _ => false,
    };
    if !within_reach {
        return;
    }
    // Land the blow: the i-frame gate, the `ClientboundDamageEventPacket`,
    // knockback, hurt/death sounds, and the corpse/loot/XP all live in
    // `mob::damage`. The attacker's id (for the damage event) and position (for the
    // knockback direction) come from the player entity.
    let attacker_id = world.get::<Profile>(attacker).map(|p| p.entity_id);
    let ctx = match (attacker_id, attacker_pos) {
        (Some(id), Some((ax, _ay, az))) => super::mob::DamageContext::player_attack(id, (ax, az)),
        // No profile/pos (shouldn't happen for a real player): fall back to an
        // attacker-less player-attack source so the hit still lands.
        _ => super::mob::DamageContext {
            attacker_id,
            attacker_xz: None,
            by_player: true,
            damage_type: "minecraft:player_attack",
        },
    };
    super::mob::damage(world, target, PLAYER_ATTACK_DAMAGE, &ctx);
    // causeFoodExhaustion(0.1) on a landed attack (`Player.attack`). Mobs have no
    // i-frames yet, so a hit within reach always connects.
    if let Some(mut food) = world.get_mut::<super::survival::FoodData>(attacker) {
        food.add_exhaustion(0.1);
    }
}

/// Find the live mob entity whose network id is `net_id`, if any. Only entities
/// carrying [`super::mob::Mob`] (and thus `Health`) are melee-damageable targets
/// today; players and item/XP entities share the id space but aren't returned.
fn find_mob_by_net_id(world: &mut World, net_id: i32) -> Option<Entity> {
    let mut q =
        world.query_filtered::<(Entity, &super::entity::NetEntity), With<super::mob::Mob>>();
    q.iter(world).find(|(_, n)| n.id == net_id).map(|(e, _)| e)
}

/// Handle a `ServerboundChatPacket`: broadcast the message to every player as an
/// (unsigned) `ClientboundPlayerChatPacket`, mirroring
/// `ServerGamePacketListenerImpl.broadcastChatMessage` →
/// `PlayerList.broadcastChatMessage`. The `globalIndex` is per-recipient
/// (vanilla's `nextChatIndex`), so the packet is rebuilt for each viewer.
///
/// The signing fields (`timestamp`/`salt`/`signature`) are forwarded into the
/// body but the message goes out **unsigned** (signature absent, link index 0):
/// see `sim::chat` for why signed forwarding is deferred.
fn on_chat(
    world: &mut World,
    sender: Entity,
    message: String,
    timestamp: i64,
    salt: i64,
    signature: Option<Vec<u8>>,
) {
    let Some((name, sender_uuid)) = world
        .get::<Profile>(sender)
        .map(|p| p.name.clone())
        .zip(world.get::<PlayerId>(sender).map(|p| p.0))
    else {
        return;
    };
    info!(%name, message = %message, signed = signature.is_some(), "chat");

    let name_component = super::text::text(name);
    let recipients: Vec<Entity> = {
        let mut q = world.query::<(Entity, &Conn)>();
        q.iter(world).map(|(e, _)| e).collect()
    };
    for recipient in recipients {
        let global_index = {
            let Some(mut state) = world.get_mut::<ChatState>(recipient) else {
                continue;
            };
            let gi = state.global_index;
            state.global_index = state.global_index.wrapping_add(1);
            gi
        };
        let bytes = chat::player_chat(
            global_index,
            sender_uuid,
            &message,
            timestamp,
            salt,
            chat::CHAT_TYPE_CHAT,
            &name_component,
        );
        send_to_self(world, recipient, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_step_matches_direction_normals() {
        // Direction 3D-data value -> unit normal (`Direction.java`, MC 26.2):
        // 0 DOWN, 1 UP, 2 NORTH(-z), 3 SOUTH(+z), 4 WEST(-x), 5 EAST(+x).
        assert_eq!(face_step(0), (0, -1, 0));
        assert_eq!(face_step(1), (0, 1, 0));
        assert_eq!(face_step(2), (0, 0, -1));
        assert_eq!(face_step(3), (0, 0, 1));
        assert_eq!(face_step(4), (-1, 0, 0));
        assert_eq!(face_step(5), (1, 0, 0));
        // Out-of-range values don't move the placement target.
        assert_eq!(face_step(6), (0, 0, 0));
        assert_eq!(face_step(-1), (0, 0, 0));
    }

    /// A minimal world with one registered player carrying `Meta`, plus the
    /// `PlayerIndex` `on_packet` resolves the sender through.
    fn one_player() -> (World, Uuid, Entity) {
        let mut world = World::new();
        let uuid = Uuid::from_u128(0x42);
        let entity = world
            .spawn((
                PlayerId(uuid),
                Profile { name: "tester".into(), entity_id: 1 },
                Meta::default(),
            ))
            .id();
        let mut index = PlayerIndex::default();
        index.0.insert(uuid, entity);
        world.insert_resource(index);
        (world, uuid, entity)
    }

    #[test]
    fn player_command_sprint_ordinals_toggle_meta() {
        // 26.2 `ServerboundPlayerCommandPacket.Action`: 1 START_SPRINTING,
        // 2 STOP_SPRINTING. Other ordinals leave sprinting untouched.
        let (mut world, uuid, entity) = one_player();

        on_packet(&mut world, uuid, Serverbound::PlayerCommand { action: 1 });
        assert!(world.get::<Meta>(entity).unwrap().sprinting);

        on_packet(&mut world, uuid, Serverbound::PlayerCommand { action: 2 });
        assert!(!world.get::<Meta>(entity).unwrap().sprinting);

        // Re-arm sprinting, then an unrelated action must not clear it.
        on_packet(&mut world, uuid, Serverbound::PlayerCommand { action: 1 });
        on_packet(&mut world, uuid, Serverbound::PlayerCommand { action: 5 }); // OPEN_INVENTORY
        assert!(world.get::<Meta>(entity).unwrap().sprinting);
    }

    #[test]
    fn player_abilities_records_only_the_flying_bit() {
        // Only bit 0x02 (flying) is meaningful serverbound; other bits are noise.
        let (mut world, uuid, entity) = one_player();

        on_packet(&mut world, uuid, Serverbound::PlayerAbilities { flags: 0x02 });
        assert!(world.get::<Meta>(entity).unwrap().flying);

        on_packet(&mut world, uuid, Serverbound::PlayerAbilities { flags: 0x00 });
        assert!(!world.get::<Meta>(entity).unwrap().flying);

        // 0x04 (an unrelated ability bit) set but 0x02 clear -> not flying.
        on_packet(&mut world, uuid, Serverbound::PlayerAbilities { flags: 0x04 });
        assert!(!world.get::<Meta>(entity).unwrap().flying);
    }

    /// Spawn a chicken (4 hp) at `pos`, returning its network id and entity. The
    /// world must already carry `NextEntityId` (and `Tick`, for the spawn path).
    fn spawn_test_chicken(world: &mut World, pos: (f64, f64, f64)) -> (i32, Entity) {
        let id = crate::sim::mob::spawn_mob(world, crate::sim::mob::MobKind::Chicken, pos);
        let e = world
            .query_filtered::<Entity, With<crate::sim::mob::Mob>>()
            .iter(world)
            .next()
            .expect("chicken spawned");
        (id, e)
    }

    #[test]
    fn attack_damages_a_mob_in_reach() {
        let (mut world, uuid, attacker) = one_player();
        world.insert_resource(NextEntityId(100));
        world.insert_resource(Tick(0));
        world.entity_mut(attacker).insert(Pos {
            x: 0.0, y: 64.0, z: 0.0, yaw: 0.0, pitch: 0.0, on_ground: true,
        });
        // A chicken one block away is well within reach.
        let (mob_id, mob) = spawn_test_chicken(&mut world, (1.0, 64.0, 0.0));
        let before = world.get::<crate::sim::mob::Health>(mob).unwrap().current;

        on_packet(&mut world, uuid, Serverbound::Attack { entity_id: mob_id });

        let after = world.get::<crate::sim::mob::Health>(mob).unwrap().current;
        assert_eq!(after, before - 1.0, "a bare-hand hit deals 1.0 damage");
    }

    #[test]
    fn attack_out_of_reach_is_ignored() {
        let (mut world, uuid, attacker) = one_player();
        world.insert_resource(NextEntityId(100));
        world.insert_resource(Tick(0));
        world.entity_mut(attacker).insert(Pos {
            x: 0.0, y: 64.0, z: 0.0, yaw: 0.0, pitch: 0.0, on_ground: true,
        });
        // A chicken 100 blocks away is far past the reach gate.
        let (mob_id, mob) = spawn_test_chicken(&mut world, (100.0, 64.0, 0.0));
        let before = world.get::<crate::sim::mob::Health>(mob).unwrap().current;

        on_packet(&mut world, uuid, Serverbound::Attack { entity_id: mob_id });

        assert_eq!(
            world.get::<crate::sim::mob::Health>(mob).unwrap().current,
            before,
            "an out-of-reach attack deals no damage"
        );
    }

    #[test]
    fn attack_on_unknown_entity_is_a_noop() {
        // No mob exists; resolving the id fails and nothing panics.
        let (mut world, uuid, attacker) = one_player();
        world.insert_resource(NextEntityId(100));
        world.entity_mut(attacker).insert(Pos {
            x: 0.0, y: 64.0, z: 0.0, yaw: 0.0, pitch: 0.0, on_ground: true,
        });
        on_packet(&mut world, uuid, Serverbound::Attack { entity_id: 4242 });
    }

    #[test]
    fn set_carried_item_validates_hotbar_range() {
        // The selected hotbar slot is 0..9; out-of-range values are ignored
        // (the wire field is a signed short, so negatives are possible).
        let (mut world, uuid, entity) = one_player();

        on_packet(&mut world, uuid, Serverbound::SetCarriedItem { slot: 3 });
        assert_eq!(
            world.get::<crate::inventory::Inventory>(entity).unwrap().selected,
            3
        );

        // Out of range: the previously-set slot is preserved.
        on_packet(&mut world, uuid, Serverbound::SetCarriedItem { slot: 9 });
        on_packet(&mut world, uuid, Serverbound::SetCarriedItem { slot: -1 });
        assert_eq!(
            world.get::<crate::inventory::Inventory>(entity).unwrap().selected,
            3
        );
    }
}
