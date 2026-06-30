//! `ItemStack` and the 26.2 network codec.
//!
//! Wire formats are taken 1:1 from the decompiled 26.2 reference — see the
//! per-method citations. Nothing here copies Mojang code; the layouts are
//! transcribed from the `StreamCodec` definitions.

use crate::protocol::buffer::{PacketReader, PacketWriter};

/// A stack of items. Data components are not modelled yet — on the wire we always
/// emit an empty `DataComponentPatch`. Emptiness is represented out-of-band as
/// `Option<ItemStack>` / `None` (the codec maps that to a zero count); a present
/// `ItemStack` always has `count >= 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    /// Numeric item id (`BuiltInRegistries.ITEM` index).
    pub id: i32,
    /// Stack size (`>= 1` for a present stack).
    pub count: i32,
}

impl ItemStack {
    #[allow(dead_code)] // convenience constructor for tests/callers building stacks by id.
    pub fn new(id: i32, count: i32) -> Self {
        Self { id, count }
    }

    /// The empty stack sentinel (`ItemStack.EMPTY`). Modelled as a zero-count air
    /// stack so the click-resolution port can pass items by value the way vanilla
    /// passes `ItemStack.EMPTY`; slot storage still uses `Option<ItemStack>`/`None`.
    pub const EMPTY: ItemStack = ItemStack { id: 0, count: 0 };

    /// Whether this stack is empty (`ItemStack.isEmpty`): air or a non-positive
    /// count.
    pub fn is_empty(&self) -> bool {
        self.id == crate::registry::item::AIR || self.count <= 0
    }

    /// The per-item maximum stack size (`ItemStack.getMaxStackSize` →
    /// `DataComponents.MAX_STACK_SIZE`, default 64). Sourced from the item
    /// registry's minimal component table.
    pub fn max_stack_size(&self) -> i32 {
        crate::registry::item::max_stack_size(self.id)
    }

    /// `ItemStack.isStackable`: a present stack whose max size exceeds one. (We do
    /// not model durability, so the damage clause is always satisfied.)
    pub fn is_stackable(&self) -> bool {
        !self.is_empty() && self.max_stack_size() > 1
    }

    /// `ItemStack.isSameItem`: same item id (components ignored — not modelled).
    pub fn same_item(a: &ItemStack, b: &ItemStack) -> bool {
        a.id == b.id
    }

    /// `ItemStack.isSameItemSameComponents`: same item and same data components.
    /// We model no components beyond count yet, so this collapses to a same-item
    /// check — documented in [`super`].
    pub fn same_item_same_components(a: &ItemStack, b: &ItemStack) -> bool {
        a.id == b.id
    }

    /// `ItemStack.matches`: equal item, count and components (used by the change
    /// detector). Components unmodelled, so id + count.
    #[allow(dead_code)] // change-detector helper for incremental sync (not on the path yet).
    pub fn matches(a: &ItemStack, b: &ItemStack) -> bool {
        a.id == b.id && a.count == b.count
    }

    /// `ItemStack.setCount`.
    pub fn set_count(&mut self, count: i32) {
        self.count = count;
    }

    /// `ItemStack.grow`.
    pub fn grow(&mut self, amount: i32) {
        self.count += amount;
    }

    /// `ItemStack.shrink`.
    pub fn shrink(&mut self, amount: i32) {
        self.count -= amount;
    }

    /// `ItemStack.copyWithCount`.
    pub fn copy_with_count(&self, count: i32) -> ItemStack {
        ItemStack { id: self.id, count }
    }

    /// `ItemStack.split`: remove up to `amount` items into a new stack, reducing
    /// this one. An empty result (or empty self) yields [`ItemStack::EMPTY`].
    pub fn split(&mut self, amount: i32) -> ItemStack {
        let take = amount.min(self.count);
        if take <= 0 || self.is_empty() {
            return ItemStack::EMPTY;
        }
        let out = self.copy_with_count(take);
        self.shrink(take);
        out
    }

    /// Normalize a possibly-empty value stack to `Option` for slot storage:
    /// `None` when empty, else `Some(self)`.
    pub fn to_option(self) -> Option<ItemStack> {
        if self.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    /// The value form of an optional slot: `None` → [`ItemStack::EMPTY`].
    pub fn from_option(slot: Option<ItemStack>) -> ItemStack {
        slot.unwrap_or(ItemStack::EMPTY)
    }
}

/// Write an `Option<ItemStack>` using the 26.2 `ItemStack.OPTIONAL_STREAM_CODEC`
/// layout (`createOptionalStreamCodec`):
///
/// * empty (`None`, or a count `<= 0`) → VarInt `0` and nothing else;
/// * otherwise → VarInt `count`, then the item id (VarInt, via the item
///   `holderRegistry` codec), then the `DataComponentPatch`. An empty patch is
///   VarInt `0` (components to add) + VarInt `0` (components to remove), per
///   `DataComponentPatch.createStreamCodec`.
pub fn write_item_stack(p: &mut PacketWriter, stack: Option<&ItemStack>) {
    match stack {
        Some(s) if s.count > 0 => {
            p.write_varint(s.count);
            p.write_varint(s.id);
            // Empty DataComponentPatch: zero added, zero removed.
            p.write_varint(0);
            p.write_varint(0);
        }
        // None, or a non-positive count: the empty encoding is a single VarInt 0.
        _ => p.write_varint(0),
    }
}

/// Read an `Option<ItemStack>`, the inverse of [`write_item_stack`]. A leading
/// count `<= 0` yields `None` (`ItemStack.EMPTY`); otherwise the item id and the
/// `DataComponentPatch` follow. We do not model components, so only the empty
/// patch (the form the vanilla creative client sends for a bare item) is
/// accepted — a non-empty patch is reported as a decode error rather than
/// silently desyncing the buffer.
pub fn read_item_stack(r: &mut PacketReader) -> std::io::Result<Option<ItemStack>> {
    let count = r.read_varint()?;
    if count <= 0 {
        return Ok(None);
    }
    let id = r.read_varint()?;
    let added = r.read_varint()?;
    let removed = r.read_varint()?;
    if added != 0 || removed != 0 {
        // We can't decode component bodies yet; refuse rather than desync. The
        // frame is its own buffer, so dropping this packet is safe.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "item stack carries data components (unsupported)",
        ));
    }
    Ok(Some(ItemStack { id, count }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_stack_round_trip_present() {
        let mut p = PacketWriter::new();
        let stack = ItemStack::new(724, 3); // diamond_sword x3
        write_item_stack(&mut p, Some(&stack));
        // count(1) + id(varint 724 = 2 bytes) + patch(0,0) = 1 + 2 + 2 = 5 bytes.
        assert_eq!(p.buf.len(), 5);
        let mut r = PacketReader::new(p.buf.freeze());
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(stack));
    }

    #[test]
    fn item_stack_round_trip_empty() {
        let mut p = PacketWriter::new();
        write_item_stack(&mut p, None);
        assert_eq!(&p.buf[..], &[0u8]); // a single VarInt 0
        let mut r = PacketReader::new(p.buf.freeze());
        assert_eq!(read_item_stack(&mut r).unwrap(), None);
    }

    #[test]
    fn non_positive_count_encodes_empty() {
        let mut p = PacketWriter::new();
        write_item_stack(&mut p, Some(&ItemStack::new(1, 0)));
        assert_eq!(&p.buf[..], &[0u8]);
    }
}
