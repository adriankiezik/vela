//! Clientbound inventory packet builders.
//!
//! Inventory/containers are a self-contained domain that will keep growing (menu
//! types, clicks, recipes), so its packet builders and ids live here rather than
//! leaking into the shared `sim::packets` module — `sim::packets` notes the same
//! split.

use bytes::Bytes;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;

use super::item_stack::{write_item_stack, ItemStack};

// ---------------------------------------------------------------------------
// Packet ids (registration order in the decompiled 26.2 `GameProtocols`).
// ---------------------------------------------------------------------------

/// `ClientboundContainerSetContentPacket` — index 18 in the clientbound PLAY
/// flow. The flow's first entry is the bundle delimiter (id 0), so the visible
/// `ADD_ENTITY` sits at id 1; `CONTAINER_SET_CONTENT` follows at 18. Verified
/// against `GameProtocols.CLIENTBOUND_TEMPLATE`.
const CB_PLAY_CONTAINER_SET_CONTENT: i32 = 18;

/// `ClientboundSetHeldSlotPacket` — index 105 in the clientbound PLAY flow
/// (`GameProtocols.CLIENTBOUND_TEMPLATE`). Tells the client which hotbar slot is
/// selected.
const CB_PLAY_SET_HELD_SLOT: i32 = 105;

/// `ClientboundContainerSetContentPacket` — overwrite a whole container's
/// contents. Layout (`StreamCodec.composite`):
///
/// * `containerId` — VarInt (`ByteBufCodecs.CONTAINER_ID` → `writeContainerId` →
///   `VarInt.write`; for the player inventory this is 0);
/// * `stateId` — VarInt;
/// * `items` — `ItemStack.OPTIONAL_LIST_STREAM_CODEC`: VarInt length then each
///   slot via the optional `ItemStack` codec;
/// * `carriedItem` — a single optional `ItemStack` (the cursor item).
#[allow(dead_code)] // scaffolding: a builder the sim will use once it pushes real contents.
pub fn container_set_content(
    window_id: i32,
    state_id: i32,
    items: &[Option<ItemStack>],
    carried: Option<&ItemStack>,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(window_id);
    p.write_varint(state_id);
    p.write_varint(items.len() as i32);
    for slot in items {
        write_item_stack(&mut p, slot.as_ref());
    }
    write_item_stack(&mut p, carried);
    frame(CB_PLAY_CONTAINER_SET_CONTENT, &p.buf)
}

/// `ClientboundSetHeldSlotPacket` — tell the client which hotbar slot (0..8) is
/// selected. A single VarInt (`ByteBufCodecs.VAR_INT`).
#[allow(dead_code)] // scaffolding: counterpart to serverbound SetCarriedItem.
pub fn set_held_slot(slot: i32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(slot);
    frame(CB_PLAY_SET_HELD_SLOT, &p.buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::item_stack::read_item_stack;
    use crate::protocol::buffer::PacketReader;

    /// Strip the `len|id` frame header and return `(id, reader-at-body)`.
    fn unframe(bytes: Bytes) -> (i32, PacketReader) {
        let mut r = PacketReader::new(bytes);
        let _len = r.read_varint().unwrap();
        let id = r.read_varint().unwrap();
        (id, r)
    }

    #[test]
    fn container_set_content_layout() {
        let items = [Some(ItemStack::new(1, 64)), None, Some(ItemStack::new(686, 1))];
        let carried = ItemStack::new(724, 1);
        let (id, mut r) = unframe(container_set_content(0, 5, &items, Some(&carried)));
        assert_eq!(id, CB_PLAY_CONTAINER_SET_CONTENT);
        assert_eq!(r.read_varint().unwrap(), 0); // window id
        assert_eq!(r.read_varint().unwrap(), 5); // state id
        assert_eq!(r.read_varint().unwrap(), 3); // item count
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(ItemStack::new(1, 64)));
        assert_eq!(read_item_stack(&mut r).unwrap(), None);
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(ItemStack::new(686, 1)));
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(carried)); // carried/cursor
    }

    #[test]
    fn set_held_slot_layout() {
        let (id, mut r) = unframe(set_held_slot(4));
        assert_eq!(id, CB_PLAY_SET_HELD_SLOT);
        assert_eq!(r.read_varint().unwrap(), 4);
    }
}
