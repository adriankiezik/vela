//! Container/menu click-mode integration tests: driving the ported
//! `AbstractContainerMenu.doClick` state machine over the wire against window 0
//! (the always-open player inventory) and asserting the authoritative
//! `ContainerSetContent` (id 18) resync it produces.
//!
//! Complements `tests/inventory.rs` (a single PICKUP) with the higher-parity
//! modes: QUICK_MOVE (shift-click), the ContainerClose cursor-return path, and a
//! number-key SWAP.

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

/// Seed container `slot` with `count` of item `id` via the creative packet.
/// ServerboundSetCreativeModeSlot (56): short slot, then an ItemStack (count
/// VarInt, id VarInt, two zero component-patch VarInts).
fn seed_creative(s: &mut TcpStream, slot: i16, id: i32, count: i32) {
    let mut creative = Vec::new();
    creative.extend_from_slice(&slot.to_be_bytes());
    write_varint(&mut creative, count);
    write_varint(&mut creative, id);
    write_varint(&mut creative, 0);
    write_varint(&mut creative, 0);
    send(s, 56, &creative);
}

/// Send a ServerboundContainerClick (18) against window 0: containerId VarInt,
/// stateId VarInt, slot short, button byte, mode VarInt. The predicted
/// changedSlots/carriedItem tail is omitted — the server re-syncs regardless.
fn click(s: &mut TcpStream, slot: i16, button: u8, mode: i32) {
    let mut click = Vec::new();
    write_varint(&mut click, 0); // container id (player inventory)
    write_varint(&mut click, 0); // state id
    click.extend_from_slice(&slot.to_be_bytes());
    click.push(button);
    write_varint(&mut click, mode);
    send(s, 18, &click);
}

/// Drain the next window-0 `ContainerSetContent` (id 18) that satisfies `pred`,
/// which is handed `(items, carried)`. Returns whether one was found.
fn expect_content(
    s: &mut TcpStream,
    mut pred: impl FnMut(&[Option<(i32, i32)>], Option<(i32, i32)>) -> bool,
) -> bool {
    drain_until(s, |id, body| {
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
        pred(&items, carried)
    })
}

/// A shift-click on a main-inventory slot quick-moves the stack to the hotbar.
/// Exercises `InventoryMenu.quickMoveStack` routing (main slots 9..36 →
/// `moveItemStackTo(36, 45)`), so stone in slot 9 lands in the first hotbar slot
/// (36) and slot 9 empties.
#[test]
fn shift_click_quick_moves_stack() {
    let addr = "127.0.0.1:25604";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Shifter", [11u8; 16]);

    // Stone x5 into main-inventory slot 9 (the first non-hotbar storage slot).
    seed_creative(&mut s, 9, 1, 5);
    std::thread::sleep(Duration::from_millis(100));

    // Shift-click (QUICK_MOVE, mode 1) slot 9. Button is ignored for QUICK_MOVE.
    click(&mut s, 9, 0, 1);
    std::thread::sleep(Duration::from_millis(200));

    let ok = expect_content(&mut s, |items, carried| {
        // The post-shift resync: slot 9 emptied, stone now in hotbar slot 36,
        // nothing on the cursor. (The pre-click seed resync has stone in slot 9,
        // so it won't match this predicate.)
        items.get(9).copied().flatten().is_none()
            && items.get(36).copied().flatten() == Some((1, 5))
            && carried.is_none()
    });
    assert!(ok, "stone quick-moved from main slot 9 to hotbar slot 36");
}

/// Closing the menu with an item on the cursor returns it to the inventory.
/// Exercises the PICKUP (`doClick` pickup branch) that lifts a stack onto the
/// cursor, then `on_container_close` → `AbstractContainerMenu.removed` →
/// `place_into_inventory`, which drops the cursor back into the first free
/// hotbar/main slot.
#[test]
fn close_with_cursor_returns_item() {
    let addr = "127.0.0.1:25605";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Closer", [12u8; 16]);

    // Stone x5 into hotbar slot 36.
    seed_creative(&mut s, 36, 1, 5);
    std::thread::sleep(Duration::from_millis(100));

    // Left-click PICKUP (mode 0) slot 36: the stack lifts onto the cursor and the
    // slot empties. Draining this resync first also consumes the pre-pickup seed
    // content (carried empty), so the close assertion below can't match it.
    click(&mut s, 36, 0, 0);
    std::thread::sleep(Duration::from_millis(200));
    let picked = expect_content(&mut s, |items, carried| {
        carried == Some((1, 5)) && items.get(36).copied().flatten().is_none()
    });
    assert!(picked, "stone lifted onto the cursor, slot 36 emptied");

    // Close the menu. ServerboundContainerClose (19): a single VarInt container id.
    let mut close = Vec::new();
    write_varint(&mut close, 0);
    send(&mut s, 19, &close);
    std::thread::sleep(Duration::from_millis(200));

    // The close resync: cursor empty, stone returned to the first free slot (the
    // now-empty hotbar slot 36).
    let returned = expect_content(&mut s, |items, carried| {
        carried.is_none() && items.get(36).copied().flatten() == Some((1, 5))
    });
    assert!(returned, "cursor item returned to the inventory on close");
}

/// A number-key press swaps the hovered slot with the target hotbar slot.
/// Exercises `doClick`'s SWAP branch → `do_swap`: with the target hotbar slot
/// empty, the hovered stack moves into it. Stone in main slot 9, SWAP with
/// button 3 (hotbar index 3 → container slot 39), lands in slot 39; slot 9
/// empties.
#[test]
fn number_key_swap_moves_to_hotbar() {
    let addr = "127.0.0.1:25606"; // own port: cargo runs tests in a file in parallel
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "Swapper", [13u8; 16]);

    // Stone x3 into main-inventory slot 9; target hotbar slot 39 stays empty.
    seed_creative(&mut s, 9, 1, 3);
    std::thread::sleep(Duration::from_millis(100));

    // SWAP (mode 2) on slot 9 with button 3 (destination hotbar index 3).
    click(&mut s, 9, 3, 2);
    std::thread::sleep(Duration::from_millis(200));

    let ok = expect_content(&mut s, |items, carried| {
        items.get(9).copied().flatten().is_none()
            && items.get(39).copied().flatten() == Some((1, 3))
            && carried.is_none()
    });
    assert!(ok, "stone swapped from main slot 9 to hotbar slot 39");
}
