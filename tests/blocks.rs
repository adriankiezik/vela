//! Block-edit integration tests: breaking and placing blocks over the wire, and
//! the visibility of those edits to other players tracking the same chunk.

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

/// A single player breaks a block and places one, and the server answers each
/// with a `BlockUpdate` (id 8) carrying the new state plus a `BlockChangedAck`
/// (id 4) echoing the client's sequence. Exercises the whole block-edit chain —
/// `play_decode` → `packet_handlers` → `world::set_block` → broadcast — over the
/// real wire.
#[test]
fn block_break_and_place_round_trip() {
    let addr = "127.0.0.1:25594";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Digger", [7u8; 16]);

    // Break the bedrock floor at (0, -64, 0) — `state_at` puts bedrock at the
    // world floor for every column, so this is solid regardless of terrain.
    // ServerboundPlayerAction (41): action VarInt (2 = STOP_DESTROY_BLOCK),
    // BlockPos long, direction byte, sequence VarInt.
    let mut dig = Vec::new();
    write_varint(&mut dig, 2);
    dig.extend_from_slice(&pack_block_pos(0, -64, 0).to_be_bytes());
    dig.push(1); // face UP (unused by the dig handler)
    write_varint(&mut dig, 5); // sequence
    send(&mut s, 41, &dig);
    std::thread::sleep(Duration::from_millis(200));

    let mut broke = false;
    let mut acked = false;
    drain_until(&mut s, |id, body| {
        match id {
            8 => {
                let node = i64::from_be_bytes(body[0..8].try_into().unwrap());
                let mut pos = 8;
                let state = read_varint_slice(body, &mut pos);
                if unpack_block_pos(node) == (0, -64, 0) {
                    assert_eq!(state, 0, "broken block becomes air (state 0)");
                    broke = true;
                }
            }
            4 => {
                let mut pos = 0;
                if read_varint_slice(body, &mut pos) == 5 {
                    acked = true;
                }
            }
            _ => {}
        }
        broke && acked
    });
    assert!(broke, "server broadcast a BlockUpdate clearing the dug block");
    assert!(acked, "server acknowledged the dig sequence");

    // Load stone (item id 1) into the selected hotbar slot (container slot 36,
    // selected index 0). ServerboundSetCreativeModeSlot (56): short slot then an
    // ItemStack (count VarInt, id VarInt, two zero patch VarInts).
    let mut creative = Vec::new();
    creative.extend_from_slice(&36i16.to_be_bytes());
    write_varint(&mut creative, 1); // count
    write_varint(&mut creative, 1); // stone
    write_varint(&mut creative, 0); // components added
    write_varint(&mut creative, 0); // components removed
    send(&mut s, 56, &creative);

    // Place it on the UP face of (0, 149, 0) — well above the terrain cap (96),
    // so the placement cell (0, 150, 0) is guaranteed air.
    // ServerboundUseItemOn (66): hand VarInt, BlockPos long, direction VarInt,
    // cursor x/y/z floats, inside bool, worldBorder bool, sequence VarInt.
    let mut place = Vec::new();
    write_varint(&mut place, 0); // main hand
    place.extend_from_slice(&pack_block_pos(0, 149, 0).to_be_bytes());
    write_varint(&mut place, 1); // face UP
    place.extend_from_slice(&0.5f32.to_be_bytes());
    place.extend_from_slice(&1.0f32.to_be_bytes());
    place.extend_from_slice(&0.5f32.to_be_bytes());
    place.push(0); // inside
    place.push(0); // world border
    write_varint(&mut place, 6); // sequence
    send(&mut s, 66, &place);
    std::thread::sleep(Duration::from_millis(200));

    let mut placed = false;
    let mut place_acked = false;
    drain_until(&mut s, |id, body| {
        match id {
            8 => {
                let node = i64::from_be_bytes(body[0..8].try_into().unwrap());
                let mut pos = 8;
                let state = read_varint_slice(body, &mut pos);
                if unpack_block_pos(node) == (0, 150, 0) {
                    assert_eq!(state, 1, "placed stone is block-state 1");
                    placed = true;
                }
            }
            4 => {
                let mut pos = 0;
                if read_varint_slice(body, &mut pos) == 6 {
                    place_acked = true;
                }
            }
            _ => {}
        }
        placed && place_acked
    });
    assert!(placed, "server broadcast a BlockUpdate for the placed stone");
    assert!(place_acked, "server acknowledged the place sequence");
}

/// A block edit by one player is visible to another player tracking the same
/// chunk. Exercises `broadcast_block_update`'s chunk-tracker routing (the update
/// goes to observers whose loaded set contains the column, not every connection).
#[test]
fn block_edit_is_visible_to_other_players() {
    let addr = "127.0.0.1:25595";
    let _server = start_server(addr);

    let mut alice = TcpStream::connect(addr).expect("connect alice");
    drive_into_play(&mut alice, addr, "Alice", [1u8; 16]);
    let mut bob = TcpStream::connect(addr).expect("connect bob");
    drive_into_play(&mut bob, addr, "Bob", [2u8; 16]);

    // Alice breaks the bedrock at spawn (chunk 0,0 — which Bob also tracks).
    let mut dig = Vec::new();
    write_varint(&mut dig, 2); // STOP_DESTROY_BLOCK
    dig.extend_from_slice(&pack_block_pos(0, -64, 0).to_be_bytes());
    dig.push(1);
    write_varint(&mut dig, 9);
    send(&mut alice, 41, &dig);
    std::thread::sleep(Duration::from_millis(300));

    // Bob must receive the BlockUpdate for that position even though Alice made
    // the edit.
    let bob_saw = drain_until(&mut bob, |id, body| {
        if id == 8 {
            let node = i64::from_be_bytes(body[0..8].try_into().unwrap());
            return unpack_block_pos(node) == (0, -64, 0);
        }
        false
    });
    assert!(bob_saw, "Bob received Alice's BlockUpdate for the shared chunk");
}
