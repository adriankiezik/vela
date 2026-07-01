//! Inventory integration tests: an inventory click runs the ported menu state
//! machine end-to-end and re-syncs the authoritative container to the client.

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

/// After seeding a slot via the creative packet, a left-click pickup must come
/// back as an authoritative `ContainerSetContent` (id 18) with the stack moved
/// onto the cursor and its origin slot cleared.
#[test]
fn inventory_click_resyncs_container() {
    let addr = "127.0.0.1:25596";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Clicker", [3u8; 16]);

    // Put stone x5 into container slot 36 (selected hotbar slot).
    let mut creative = Vec::new();
    creative.extend_from_slice(&36i16.to_be_bytes());
    write_varint(&mut creative, 5); // count
    write_varint(&mut creative, 1); // stone
    write_varint(&mut creative, 0);
    write_varint(&mut creative, 0);
    send(&mut s, 56, &creative);

    // Left-click (button 0) PICKUP (mode 0) on slot 36 of the player inventory
    // (container id 0). ServerboundContainerClick (18): containerId VarInt,
    // stateId VarInt, slot short, button byte, mode VarInt. The predicted
    // changedSlots/carriedItem tail is omitted — the server re-syncs regardless.
    let mut click = Vec::new();
    write_varint(&mut click, 0); // container id (player inventory)
    write_varint(&mut click, 0); // state id
    click.extend_from_slice(&36i16.to_be_bytes()); // slot
    click.push(0); // button
    write_varint(&mut click, 0); // mode PICKUP
    send(&mut s, 18, &click);
    std::thread::sleep(Duration::from_millis(200));

    // Find the ContainerSetContent whose cursor now holds the stone stack.
    let mut slot36: Option<(i32, i32)> = Some((0, 0)); // sentinel != None
    let got = drain_until(&mut s, |id, body| {
        if id != 18 {
            return false;
        }
        let mut pos = 0;
        if read_varint_slice(body, &mut pos) != 0 {
            return false; // not the player inventory
        }
        let _state_id = read_varint_slice(body, &mut pos);
        let count = read_varint_slice(body, &mut pos);
        let mut items = Vec::with_capacity(count as usize);
        for _ in 0..count {
            items.push(read_item(body, &mut pos));
        }
        let carried = read_item(body, &mut pos);
        if carried == Some((1, 5)) {
            slot36 = items.get(36).copied().flatten();
            return true;
        }
        false
    });
    assert!(got, "received a ContainerSetContent with stone on the cursor");
    assert_eq!(slot36, None, "the picked-up slot 36 is now empty");
}
