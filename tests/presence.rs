//! Multiplayer presence integration tests: players seeing each other join and
//! move, and being despawned for the others on disconnect.

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

/// Two players must see each other join (player list + entity spawn) and see
/// each other move (relative movement + head rotation broadcasts). Drives two
/// real connections against the server binary.
#[test]
fn two_players_see_each_other() {
    let addr = "127.0.0.1:25593";
    let _server = start_server(addr);

    let mut alice = TcpStream::connect(addr).expect("connect alice");
    drive_into_play(&mut alice, addr, "Alice", [1u8; 16]);

    // Bob joins second; his entity id is 2 (Alice took 1).
    let mut bob = TcpStream::connect(addr).expect("connect bob");
    drive_into_play(&mut bob, addr, "Bob", [2u8; 16]);

    // Bob (the newcomer) must be told about Alice (already online): a
    // PlayerInfoUpdate followed by an AddEntity for Alice's entity id 1.
    let mut bob_saw_alice_info = false;
    for _ in 0..600 {
        let (id, body) = recv(&mut bob);
        if id == 70 {
            bob_saw_alice_info = true;
        }
        if id == 1 {
            let mut pos = 0;
            assert_eq!(read_varint_slice(&body, &mut pos), 1, "Alice's entity id");
            pos += 16; // skip uuid
            assert_eq!(read_varint_slice(&body, &mut pos), 156, "player entity type");
            break;
        }
    }
    assert!(bob_saw_alice_info, "Bob received PlayerInfoUpdate for Alice");

    // Alice (already online) must be told about Bob joining: PlayerInfoUpdate
    // then AddEntity for Bob's entity id 2.
    let mut alice_saw_bob_info = false;
    let mut alice_saw_bob_spawn = false;
    for _ in 0..600 {
        let (id, body) = recv(&mut alice);
        if id == 70 {
            alice_saw_bob_info = true;
        }
        if id == 1 {
            let mut pos = 0;
            assert_eq!(read_varint_slice(&body, &mut pos), 2, "Bob's entity id");
            pos += 16;
            assert_eq!(read_varint_slice(&body, &mut pos), 156, "player entity type");
            alice_saw_bob_spawn = true;
            break;
        }
    }
    assert!(alice_saw_bob_info, "Alice received PlayerInfoUpdate for Bob");
    assert!(alice_saw_bob_spawn, "Alice received AddEntity for Bob");

    // Bob moves and turns. Alice should receive a position-carrying packet and a
    // head-rotation packet, both for Bob's entity id 2.
    let mut move_body = Vec::new();
    move_body.extend_from_slice(&1.5f64.to_be_bytes()); // x
    move_body.extend_from_slice(&64.0f64.to_be_bytes()); // y
    move_body.extend_from_slice(&(-3.5f64).to_be_bytes()); // z
    move_body.extend_from_slice(&90.0f32.to_be_bytes()); // yaw
    move_body.extend_from_slice(&12.0f32.to_be_bytes()); // pitch
    move_body.push(1); // flags: on ground
    send(&mut bob, 31, &move_body); // ServerboundMovePlayerPosRot

    // Give the simulation a few ticks to broadcast.
    std::thread::sleep(Duration::from_millis(300));

    let mut saw_position = false;
    let mut saw_head = false;
    for _ in 0..1000 {
        let (id, body) = recv(&mut alice);
        match id {
            // MoveEntityPosRot or EntityPositionSync for Bob.
            54 | 35 => {
                let mut pos = 0;
                if read_varint_slice(&body, &mut pos) == 2 {
                    saw_position = true;
                }
            }
            // RotateHead for Bob — proves the body/head yaw propagated.
            83 => {
                let mut pos = 0;
                if read_varint_slice(&body, &mut pos) == 2 {
                    saw_head = true;
                }
            }
            _ => {}
        }
        if saw_position && saw_head {
            break;
        }
    }
    assert!(saw_position, "Alice received Bob's position update");
    assert!(saw_head, "Alice received Bob's head rotation");
}

/// When a player disconnects, everyone still online is told to drop them —
/// `PlayerInfoRemove` (id 69) from the tab list and `RemoveEntities` (id 77) for
/// their entity. Exercises the `on_left` cleanup path.
#[test]
fn disconnect_despawns_for_others() {
    let addr = "127.0.0.1:25597";
    let _server = start_server(addr);

    let mut alice = TcpStream::connect(addr).expect("connect alice");
    drive_into_play(&mut alice, addr, "Alice", [1u8; 16]);
    let mut bob = TcpStream::connect(addr).expect("connect bob");
    drive_into_play(&mut bob, addr, "Bob", [2u8; 16]);

    // Make sure Alice has registered Bob (his AddEntity, id 1 for entity id 2)
    // before he leaves, so the despawn is a genuine removal.
    let saw_bob = drain_until(&mut alice, |id, body| {
        if id == 1 {
            let mut pos = 0;
            return read_varint_slice(body, &mut pos) == 2;
        }
        false
    });
    assert!(saw_bob, "Alice saw Bob spawn before he disconnected");

    // Bob drops his connection; the server should detect EOF and broadcast his
    // removal to Alice.
    drop(bob);
    std::thread::sleep(Duration::from_millis(500));

    let mut saw_info_remove = false;
    let mut saw_remove_entity = false;
    drain_until(&mut alice, |id, body| {
        match id {
            69 => saw_info_remove = true, // PlayerInfoRemove
            77 => {
                // RemoveEntities: VarInt count then VarInt entity ids.
                let mut pos = 0;
                let count = read_varint_slice(body, &mut pos);
                for _ in 0..count {
                    if read_varint_slice(body, &mut pos) == 2 {
                        saw_remove_entity = true;
                    }
                }
            }
            _ => {}
        }
        saw_info_remove && saw_remove_entity
    });
    assert!(saw_info_remove, "Alice received PlayerInfoRemove for Bob");
    assert!(saw_remove_entity, "Alice received RemoveEntities for Bob's entity");
}
