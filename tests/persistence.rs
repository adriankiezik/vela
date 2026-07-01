//! Player-data persistence integration test: a player's saved position,
//! orientation, held slot, and inventory survive a disconnect/rejoin round-trip.
//!
//! Exercises `sim::player_lifecycle::save_player_data` (on `on_left`) writing
//! `playerdata/<uuid>.dat`, and `on_joined` reloading it — the restored state is
//! observed over the wire in the *second* connection's join sequence
//! (`PlayerPosition` id 72, `ContainerSetContent` id 18 window 0, `SetHeldSlot`
//! id 105), all with the SAME uuid against the SAME still-running server.

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

/// Read a big-endian f64 out of a body slice at `pos`, advancing it.
fn read_f64(b: &[u8], pos: &mut usize) -> f64 {
    let v = f64::from_be_bytes(b[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    v
}

/// Read a big-endian f32 out of a body slice at `pos`, advancing it.
fn read_f32(b: &[u8], pos: &mut usize) -> f32 {
    let v = f32::from_be_bytes(b[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    v
}

/// A returning player must be spawned back where they left, holding the slot
/// they had selected, with the inventory they saved. Client A mutates its state
/// then disconnects (persisting via `on_left`); client B rejoins with the same
/// uuid against the same server and the restored state shows up in its join
/// stream. Mirrors vanilla `PlayerDataStorage` save-on-leave / load-on-join.
#[test]
fn player_data_survives_rejoin() {
    let addr = "127.0.0.1:25603";
    // ONE server kept alive across both connections so the temp workdir (and its
    // `playerdata/`) is shared; killed only when `_server` drops at test end.
    let _server = start_server(addr);
    let uuid = [9u8; 16];

    // --- Client A: give the player a distinctive position + inventory + slot ---
    {
        let mut a = TcpStream::connect(addr).expect("connect A");
        drive_into_play(&mut a, addr, "Returner", uuid);

        // Put stone (item id 1) x7 into container slot 36 (the first hotbar slot).
        // ServerboundSetCreativeModeSlot (56): short slot, then an ItemStack
        // (count VarInt, id VarInt, two zero component-patch VarInts).
        let mut creative = Vec::new();
        creative.extend_from_slice(&36i16.to_be_bytes());
        write_varint(&mut creative, 7); // count
        write_varint(&mut creative, 1); // stone
        write_varint(&mut creative, 0); // components added
        write_varint(&mut creative, 0); // components removed
        send(&mut a, 56, &creative);

        // Select hotbar slot 4 (non-default). ServerboundSetCarriedItem (53): a
        // single signed short slot index; the sim sets `Inventory.selected`.
        let mut carried = Vec::new();
        carried.extend_from_slice(&4i16.to_be_bytes());
        send(&mut a, 53, &carried);

        // Move to a distinctive absolute position/orientation.
        // ServerboundMovePlayerPosRot (31): x/y/z doubles, yaw/pitch floats, flags
        // byte (bit 0 = on-ground).
        let mut mv = Vec::new();
        mv.extend_from_slice(&10.5f64.to_be_bytes()); // x
        mv.extend_from_slice(&70.0f64.to_be_bytes()); // y
        mv.extend_from_slice(&(-8.5f64).to_be_bytes()); // z
        mv.extend_from_slice(&45.0f32.to_be_bytes()); // yaw
        mv.extend_from_slice(&10.0f32.to_be_bytes()); // pitch
        mv.push(1); // on ground
        send(&mut a, 31, &mv);

        // Let the sim apply the move/creative/slot changes before disconnecting.
        std::thread::sleep(Duration::from_millis(300));
        drop(a);
    }

    // Give `on_left` a moment to persist `<uuid>.dat` after the EOF is detected.
    std::thread::sleep(Duration::from_millis(500));

    // --- Client B: rejoin with the SAME uuid; observe the restored state --------
    let mut b = TcpStream::connect(addr).expect("connect B");
    // `drive_into_play` enters Play and acks teleport 1 WITHOUT inspecting the
    // join packets, so the `PlayerPosition`/`ContainerSetContent`/`SetHeldSlot`
    // are still queued in the socket for us to drain and assert against.
    drive_into_play(&mut b, addr, "Returner", uuid);

    // 1) The spawn PlayerPosition (id 72) must carry the restored coordinates.
    //    Body: teleport-id VarInt, x/y/z doubles, dx/dy/dz doubles, yaw/pitch
    //    floats, i32 relative flags.
    let mut got_pos = false;
    let ok = drain_until(&mut b, |id, body| {
        if id != 72 {
            return false;
        }
        let mut pos = 0;
        let teleport_id = read_varint_slice(body, &mut pos);
        assert_eq!(teleport_id, 1, "spawn teleport id");
        let x = read_f64(body, &mut pos);
        let y = read_f64(body, &mut pos);
        let z = read_f64(body, &mut pos);
        let _dx = read_f64(body, &mut pos);
        let _dy = read_f64(body, &mut pos);
        let _dz = read_f64(body, &mut pos);
        let yaw = read_f32(body, &mut pos);
        let pitch = read_f32(body, &mut pos);
        assert!((x - 10.5).abs() < 1e-6, "restored x, got {x}");
        assert!((y - 70.0).abs() < 1e-6, "restored y, got {y}");
        assert!((z - (-8.5)).abs() < 1e-6, "restored z, got {z}");
        assert!((yaw - 45.0).abs() < 1e-3, "restored yaw, got {yaw}");
        assert!((pitch - 10.0).abs() < 1e-3, "restored pitch, got {pitch}");
        got_pos = true;
        true
    });
    assert!(ok && got_pos, "received the restored spawn PlayerPosition");

    // 2) The join-stream ContainerSetContent (id 18, window 0) must hold stone x7
    //    back in slot 36.
    let mut slot36: Option<(i32, i32)> = None;
    let inv_ok = drain_until(&mut b, |id, body| {
        if id != 18 {
            return false;
        }
        let mut pos = 0;
        if read_varint_slice(body, &mut pos) != 0 {
            return false; // not the player inventory (window 0)
        }
        let _state_id = read_varint_slice(body, &mut pos);
        let count = read_varint_slice(body, &mut pos);
        let mut items = Vec::with_capacity(count as usize);
        for _ in 0..count {
            items.push(read_item(body, &mut pos));
        }
        slot36 = items.get(36).copied().flatten();
        true
    });
    assert!(inv_ok, "received the restored ContainerSetContent");
    assert_eq!(slot36, Some((1, 7)), "stone x7 restored in slot 36");

    // 3) The SetHeldSlot (id 105) must reflect the saved selected slot 4.
    let mut selected = -1;
    let held_ok = drain_until(&mut b, |id, body| {
        if id != 105 {
            return false;
        }
        let mut pos = 0;
        selected = read_varint_slice(body, &mut pos);
        true
    });
    assert!(held_ok, "received the restored SetHeldSlot");
    assert_eq!(selected, 4, "selected hotbar slot restored");
}
