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
    #[allow(dead_code)] // scaffolding: convenience constructor for future callers/tests.
    pub fn new(id: i32, count: i32) -> Self {
        Self { id, count }
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
