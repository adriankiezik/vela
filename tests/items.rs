//! Dropped-item integration tests: the survival block-drop → item-entity →
//! pickup loop (and the same-item merge) exercised end-to-end over the wire.
//!
//! These drive the real server binary through the whole gameplay chain that a
//! vanilla client would trigger: `ServerboundPlayerAction` (dig) →
//! `packet_handlers` spawning an `ItemEntity` via `world::block_drop::drops_for`
//! → `sim::item_tick` physics/pickup/merge → the clientbound `AddEntity` (1),
//! `SetEntityData` (99), `TakeItemEntity` (124) and `RemoveEntities` (77)
//! broadcasts. The server runs default-gamemode survival, so breaking a block
//! drops its item (creative would destroy without drops).

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

/// Entity-type registry ids we discriminate on the wire. The item entity type is
/// `minecraft:item` = 71 in the 26.2 registry (see `sim::entity`); the player type
/// is 156. `AddEntity` (id 1) carries this type id right after the 16-byte UUID.
const ENTITY_TYPE_ITEM: i32 = 71;
/// `dirt` item id — the drop from both grass_block (state 9, no Silk Touch) and
/// dirt (state 10). See `world::block_drop`.
const ITEM_DIRT: i32 = 55;

/// Read the player's spawn Y off the `ClientboundPlayerPosition` (id 72) teleport
/// that arrives on entering Play. Layout (`player_position`): teleport id (VarInt)
/// then the position (three f64). `drive_into_play` blind-acks teleport id 1
/// without reading the packet, so it is still queued for us to parse — this is how
/// the fake client learns the surface height it never computed itself.
fn read_spawn_y(s: &mut TcpStream) -> f64 {
    let mut y = None;
    let got = drain_until(s, |id, body| {
        if id != 72 {
            return false;
        }
        let mut pos = 0;
        let _teleport_id = read_varint_slice(body, &mut pos);
        pos += 8; // skip x
        y = Some(f64::from_be_bytes(body[pos..pos + 8].try_into().unwrap()));
        true
    });
    assert!(got, "received the spawn PlayerPosition teleport");
    y.unwrap()
}

/// Break the block at `(x, y, z)` by sending the dig completion the vanilla client
/// emits when a dig finishes. `ServerboundPlayerAction` (41): action VarInt
/// (2 = STOP_DESTROY_BLOCK), BlockPos long, direction byte, sequence VarInt. In
/// survival this both clears the block and spawns its drops.
fn break_block(s: &mut TcpStream, x: i32, y: i32, z: i32, sequence: i32) {
    let mut dig = Vec::new();
    write_varint(&mut dig, 2); // STOP_DESTROY_BLOCK
    dig.extend_from_slice(&pack_block_pos(x, y, z).to_be_bytes());
    dig.push(1); // face UP (unused by the dig handler)
    write_varint(&mut dig, sequence);
    send(s, 41, &dig);
}

/// Move the player's authoritative server Pos. `ServerboundMovePlayerPosRot` (31):
/// double x, y, z, float yaw, pitch, byte flags (bit0 = on_ground). Same layout
/// `tests/presence.rs` uses.
fn move_to(s: &mut TcpStream, x: f64, y: f64, z: f64) {
    let mut body = Vec::new();
    body.extend_from_slice(&x.to_be_bytes());
    body.extend_from_slice(&y.to_be_bytes());
    body.extend_from_slice(&z.to_be_bytes());
    body.extend_from_slice(&0.0f32.to_be_bytes()); // yaw
    body.extend_from_slice(&0.0f32.to_be_bytes()); // pitch
    body.push(1); // flags: on ground
    send(s, 31, &body);
}

/// Parse an `AddEntity` (id 1) body of a `minecraft:item`, returning
/// `(entity_id, x, y, z)`. Layout (`add_entity`): entity id (VarInt), 16-byte
/// UUID, entity type (VarInt), then position (three f64). Returns `None` for a
/// non-item spawn (e.g. a player, type 156).
fn parse_item_add(body: &[u8]) -> Option<(i32, f64, f64, f64)> {
    let mut pos = 0;
    let eid = read_varint_slice(body, &mut pos);
    pos += 16; // skip UUID
    let type_id = read_varint_slice(body, &mut pos);
    if type_id != ENTITY_TYPE_ITEM {
        return None;
    }
    let x = f64::from_be_bytes(body[pos..pos + 8].try_into().unwrap());
    let y = f64::from_be_bytes(body[pos + 8..pos + 16].try_into().unwrap());
    let z = f64::from_be_bytes(body[pos + 16..pos + 24].try_into().unwrap());
    Some((eid, x, y, z))
}

/// Breaking a block in survival drops its item, and a player standing on the drop
/// collects it: the drop appears as an `AddEntity` (item type), the pickup fires a
/// `TakeItemEntity` (id 124) crediting the collector, the item is removed with a
/// `RemoveEntities` (id 77), and the collector's inventory re-syncs via
/// `ContainerSetContent` (id 18) now holding the item. Exercises
/// `packet_handlers` (drop spawn) → `item_tick` (physics + pickup) end-to-end.
#[test]
fn break_drops_item_and_player_picks_it_up() {
    let addr = "127.0.0.1:25601";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Miner", [11u8; 16]);

    // The player spawns at (0, surface+1, 0) standing on the grass block at
    // (0, surface, 0). Learn `surface` from the spawn teleport rather than
    // recomputing the terrain noise here.
    let spawn_y = read_spawn_y(&mut s);
    let surface_y = spawn_y as i32 - 1; // grass block y = feet - 1

    // Break the grass block the player is standing on. grass_block (state 9) drops
    // dirt (item 55), and the drop spawns at that block's centre (see
    // `drop_position`), i.e. right in the player's column.
    break_block(&mut s, 0, surface_y, 0, 1);
    std::thread::sleep(Duration::from_millis(200));

    // The drop must arrive as an item-type AddEntity. Capture its entity id (the
    // handle we later expect a TakeItemEntity + RemoveEntities for).
    let mut item_eid = None;
    let saw_item = drain_until(&mut s, |id, body| {
        if id != 1 {
            return false;
        }
        if let Some((eid, _x, _y, _z)) = parse_item_add(body) {
            item_eid = Some(eid);
            return true;
        }
        false
    });
    assert!(saw_item, "server spawned an item entity for the block drop");
    let item_eid = item_eid.unwrap();

    // Stand the player's server Pos down in the broken cell, on top of the drop.
    // The item settles onto the dirt at (0, surface-1, 0) → rest y = surface, so
    // placing the player's feet at `surface` puts the drop inside the pickup box
    // (the player AABB inflated by (1.0, 0.5, 1.0)).
    move_to(&mut s, 0.5, surface_y as f64, 0.5);

    // Pickup is gated by DEFAULT_PICKUP_DELAY = 10 ticks (0.5 s at 20 TPS); give it
    // comfortably more than that plus settle time.
    std::thread::sleep(Duration::from_millis(700));

    // Assert the three pickup signals arrive: the take animation crediting the
    // player (entity id 1, first to join), the inventory re-sync holding the dirt,
    // and the item entity's removal (a full pickup).
    let mut saw_take = false;
    let mut saw_content = false;
    let mut saw_remove = false;
    drain_until(&mut s, |id, body| {
        match id {
            // TakeItemEntity (124): itemEntityId VarInt, collectorId VarInt,
            // amount VarInt.
            124 => {
                let mut pos = 0;
                let taken = read_varint_slice(body, &mut pos);
                let collector = read_varint_slice(body, &mut pos);
                if taken == item_eid {
                    assert_eq!(collector, 1, "the collecting player's entity id");
                    saw_take = true;
                }
            }
            // ContainerSetContent (18) for the player inventory (container 0):
            // stateId, count, then that many items, then the carried item.
            18 => {
                let mut pos = 0;
                if read_varint_slice(body, &mut pos) != 0 {
                    return false; // not the player inventory
                }
                let _state_id = read_varint_slice(body, &mut pos);
                let count = read_varint_slice(body, &mut pos);
                for _ in 0..count {
                    if read_item(body, &mut pos) == Some((ITEM_DIRT, 1)) {
                        saw_content = true;
                    }
                }
            }
            // RemoveEntities (77): count VarInt, then that many entity id VarInts.
            77 => {
                let mut pos = 0;
                let count = read_varint_slice(body, &mut pos);
                for _ in 0..count {
                    if read_varint_slice(body, &mut pos) == item_eid {
                        saw_remove = true;
                    }
                }
            }
            _ => {}
        }
        saw_take && saw_content && saw_remove
    });
    assert!(saw_take, "TakeItemEntity credited the player for the drop");
    assert!(saw_content, "the player's inventory re-synced holding the dirt drop");
    assert!(saw_remove, "the item entity was removed after a full pickup");
}

/// Two same-item drops that come to rest in the same column merge into a single
/// stack: one of the two item entities is consumed (a `RemoveEntities`, id 77)
/// while the other survives. Exercises `item_tick::merge_pass`
/// (`ItemEntity.mergeWithNeighbours`).
///
/// SIMPLIFICATION (documented): rather than assert the survivor's merged count via
/// its `SetEntityData` (id 99) metadata — fiddly to pin on the wire and cadence-
/// dependent — we assert the reliable structural signal of a merge: exactly one of
/// the two spawned item entities is removed, with no player in pickup range to
/// cause that removal any other way. Breaking two blocks straight down in the same
/// column guarantees both drops land at the same spot (drop jitter is only ±0.25
/// around the block centre, well inside the merge search box of inflate(0.5)), so
/// the merge geometry is hit deterministically regardless of jitter.
#[test]
fn two_drops_merge_into_one() {
    let addr = "127.0.0.1:25602";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Merger", [12u8; 16]);

    let spawn_y = read_spawn_y(&mut s);
    let surface_y = spawn_y as i32 - 1;

    // Break two blocks straight down in the player's column: the grass block at
    // (0, surface, 0) → dirt, then the dirt at (0, surface-1, 0) → dirt. Both
    // drops are dirt (item 55) and both settle onto the dirt at (0, surface-2, 0)
    // at rest y = surface-1, in the same (0.5, 0.5) column — inside each other's
    // merge box. The player stays up at spawn (feet surface+1), so the drops are
    // far below the pickup box and never get collected (keeping chunk (0,0)
    // loaded so we still receive the item broadcasts).
    break_block(&mut s, 0, surface_y, 0, 1);
    break_block(&mut s, 0, surface_y - 1, 0, 2);

    std::thread::sleep(Duration::from_millis(200));

    // Both drops must spawn as item-type AddEntity packets. Collect their ids.
    let mut item_ids: Vec<i32> = Vec::new();
    drain_until(&mut s, |id, body| {
        if id == 1 {
            if let Some((eid, _x, _y, _z)) = parse_item_add(body) {
                if !item_ids.contains(&eid) {
                    item_ids.push(eid);
                }
            }
        }
        item_ids.len() >= 2
    });
    assert!(
        item_ids.len() >= 2,
        "both block drops spawned as item entities (got {})",
        item_ids.len()
    );

    // Let the items settle and the merge cadence fire (resting items merge every
    // 40 ticks = 2 s; give margin).
    std::thread::sleep(Duration::from_millis(2500));

    // The merge's RemoveEntities has already been sent (and buffered) by now, so the
    // final scan only needs to drain the backlog — not wait ~20 s for the server to
    // drop this deliberately-idle client on the keep-alive timeout, which is what
    // `drain_until`'s stream-end fallback would otherwise block on. Drop the read
    // timeout to a small idle gap: the backlog streams back-to-back, and once it is
    // exhausted the next packet (a 1 Hz SetTime) is far enough out that the timeout
    // fires and the scan returns promptly.
    s.set_read_timeout(Some(Duration::from_millis(250))).unwrap();

    // Exactly one of the two item entities is removed by the merge (folded into the
    // other). No player is in pickup range, and the item lifetime is 6000 ticks, so
    // a removal here can only be the merge.
    let mut removed: Vec<i32> = Vec::new();
    drain_until(&mut s, |id, body| {
        if id == 77 {
            let mut pos = 0;
            let count = read_varint_slice(body, &mut pos);
            for _ in 0..count {
                let eid = read_varint_slice(body, &mut pos);
                if item_ids.contains(&eid) && !removed.contains(&eid) {
                    removed.push(eid);
                }
            }
        }
        false // drain the whole backlog; we count removals below
    });
    assert_eq!(
        removed.len(),
        1,
        "exactly one of the two drops was consumed by the merge (removed {removed:?})"
    );
}
