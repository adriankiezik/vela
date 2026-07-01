//! `SynchedEntityData` — the typed entity-data model and its wire encoding.
//!
//! Mirrors vanilla `net.minecraft.network.syncher`: an entity carries a set of
//! [`DataItem`]s, each a numeric accessor index paired with a typed value. The
//! `ClientboundSetEntityDataPacket` serializes them as a packed list — per entry
//! the accessor index (`u8`), the serializer type id (VarInt), then the value —
//! terminated by the index `0xFF` (`ClientboundSetEntityDataPacket.EOF_MARKER`).
//!
//! Nothing here copies Mojang code; the layouts are transcribed from the 26.2
//! `EntityDataSerializers` registration order and the per-serializer
//! `StreamCodec`s.

use crate::inventory::{write_item_stack, ItemStack};
use crate::protocol::buffer::PacketWriter;

/// The index byte that terminates the packed metadata list
/// (`ClientboundSetEntityDataPacket.EOF_MARKER`).
pub const EOF_MARKER: u8 = 0xFF;

// `EntityDataSerializers` registration order gives each serializer its wire type
// id (the order of the `registerSerializer(..)` calls in the class initializer):
// BYTE=0, INT=1, LONG=2, FLOAT=3, STRING=4, COMPONENT=5, OPTIONAL_COMPONENT=6,
// ITEM_STACK=7, BOOLEAN=8, ROTATIONS=9, BLOCK_POS=10, OPTIONAL_BLOCK_POS=11, …,
// POSE=20. Only the ids we actually emit are named here.
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_INT: i32 = 1;
const SERIALIZER_FLOAT: i32 = 3;
const SERIALIZER_ITEM_STACK: i32 = 7;
const SERIALIZER_BOOLEAN: i32 = 8;
const SERIALIZER_OPTIONAL_BLOCK_POS: i32 = 11;
const SERIALIZER_POSE: i32 = 20;

/// One typed entity-data value (`SynchedEntityData.DataValue`'s value half). Each
/// variant knows its `EntityDataSerializers` type id and how to encode itself,
/// exactly matching the corresponding vanilla serializer's `StreamCodec`.
///
/// The set covers the serializers Vela currently emits (items, XP orbs, the
/// player shared-flags/pose). Others in the vanilla table are intentionally left
/// out until a caller needs them — add the variant plus its `serializer_id` /
/// `encode` arms rather than emitting a raw index.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // several variants land with their first entity user (mobs, etc.).
pub enum DataValue {
    /// `EntityDataSerializers.BYTE` — e.g. the shared-flags bitfield.
    Byte(u8),
    /// `EntityDataSerializers.INT` — a `VAR_INT` (e.g. an XP orb's value).
    Int(i32),
    /// `EntityDataSerializers.FLOAT`.
    Float(f32),
    /// `EntityDataSerializers.ITEM_STACK` — the optional-item-stack codec
    /// (an empty stack encodes as a single VarInt `0`).
    ItemStack(Option<ItemStack>),
    /// `EntityDataSerializers.BOOLEAN`.
    Boolean(bool),
    /// `EntityDataSerializers.OPTIONAL_BLOCK_POS` — a present-bool then the packed
    /// `BlockPos` long when present.
    OptionalBlockPos(Option<(i32, i32, i32)>),
    /// `EntityDataSerializers.POSE` — the `Pose` enum id as a VarInt.
    Pose(i32),
}

impl DataValue {
    /// This value's serializer type id (its index in the `EntityDataSerializers`
    /// registration order), written as a VarInt after the accessor index.
    pub fn serializer_id(&self) -> i32 {
        match self {
            DataValue::Byte(_) => SERIALIZER_BYTE,
            DataValue::Int(_) => SERIALIZER_INT,
            DataValue::Float(_) => SERIALIZER_FLOAT,
            DataValue::ItemStack(_) => SERIALIZER_ITEM_STACK,
            DataValue::Boolean(_) => SERIALIZER_BOOLEAN,
            DataValue::OptionalBlockPos(_) => SERIALIZER_OPTIONAL_BLOCK_POS,
            DataValue::Pose(_) => SERIALIZER_POSE,
        }
    }

    /// Encode just the value body (the serializer's `StreamCodec.encode`).
    fn encode(&self, p: &mut PacketWriter) {
        match self {
            DataValue::Byte(v) => p.write_u8(*v),
            DataValue::Int(v) => p.write_varint(*v),
            DataValue::Float(v) => p.write_f32(*v),
            DataValue::ItemStack(stack) => write_item_stack(p, stack.as_ref()),
            DataValue::Boolean(v) => p.write_bool(*v),
            DataValue::OptionalBlockPos(pos) => match pos {
                Some((x, y, z)) => {
                    p.write_bool(true);
                    p.write_block_pos(*x, *y, *z);
                }
                None => p.write_bool(false),
            },
            DataValue::Pose(id) => p.write_varint(*id),
        }
    }
}

/// One accessor→value binding (`SynchedEntityData.DataItem`): the accessor's
/// numeric index and its current value.
#[derive(Debug, Clone, PartialEq)]
pub struct DataItem {
    /// The accessor index (`EntityDataAccessor.id`), assigned in `defineId`
    /// order down the entity class hierarchy (`Entity` uses 0..=7).
    pub index: u8,
    pub value: DataValue,
    /// Whether this binding changed since the last `pack_dirty`
    /// (`SynchedEntityData.DataItem.dirty`). A freshly-set item is dirty until
    /// the next incremental flush emits it.
    dirty: bool,
}

/// An entity's synchronized data — the list of accessor bindings that
/// `ClientboundSetEntityDataPacket` serializes. Vela only tracks the non-default
/// entries it explicitly sets (vanilla's `getNonDefaultValues`), which is all the
/// client needs to render.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityData {
    items: Vec<DataItem>,
}

// The builder half (`new`/`set`) is exercised by the spawn API and tests; the
// wire half (`write_packed`) is on the live join-replay path. Silence dead-code
// for the constructors until the spawn API gains an in-game trigger.
#[allow(dead_code)]
impl EntityData {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Set an accessor's value, replacing any existing binding at that index.
    /// Marks the binding dirty only when the value actually changes (matching
    /// `SynchedEntityData.set`, which compares before flagging), so an unchanged
    /// re-set produces no incremental update. Returns `&mut self` for chaining.
    pub fn set(&mut self, index: u8, value: DataValue) -> &mut Self {
        match self.items.iter_mut().find(|it| it.index == index) {
            Some(it) => {
                if it.value != value {
                    it.value = value;
                    it.dirty = true;
                }
            }
            None => self.items.push(DataItem { index, value, dirty: true }),
        }
        self
    }

    /// Whether any bindings are present.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The current bindings, in insertion order.
    pub fn items(&self) -> &[DataItem] {
        &self.items
    }

    /// Write the full packed metadata list into `p`: every binding as `index`
    /// (u8) + `serializer_id` (VarInt) + value, then the `0xFF` terminator. This
    /// is the spawn/join-replay body of `ClientboundSetEntityDataPacket` after the
    /// entity id — vanilla `getNonDefaultValues` packed in full (a "pack all").
    /// It leaves dirty flags untouched; use [`Self::write_dirty`] for incremental
    /// updates.
    pub fn write_packed(&self, p: &mut PacketWriter) {
        write_items(self.items.iter(), p);
    }

    /// Emit only the bindings changed since the last flush and clear their dirty
    /// flags (`SynchedEntityData.packDirty` semantics), returning the emitted
    /// entries. An incremental `ClientboundSetEntityDataPacket` should be sent
    /// only when the result is non-empty. Note this is *not* wired into any tick
    /// loop yet — it's the ready-to-use API for post-spawn metadata updates.
    pub fn pack_dirty(&mut self) -> Vec<DataItem> {
        let mut out = Vec::new();
        for it in self.items.iter_mut() {
            if it.dirty {
                it.dirty = false;
                out.push(it.clone());
            }
        }
        out
    }

    /// Like [`Self::pack_dirty`], but writes the changed bindings straight into
    /// `p` as the `ClientboundSetEntityDataPacket` body (packed entries + `0xFF`
    /// terminator) and clears their dirty flags. Returns whether anything was
    /// emitted, so the caller can skip sending an empty update.
    pub fn write_dirty(&mut self, p: &mut PacketWriter) -> bool {
        let mut any = false;
        for it in self.items.iter_mut() {
            if it.dirty {
                it.dirty = false;
                p.write_u8(it.index);
                p.write_varint(it.value.serializer_id());
                it.value.encode(p);
                any = true;
            }
        }
        p.write_u8(EOF_MARKER);
        any
    }
}

/// Shared body writer: packed entries (`index` + `serializer_id` + value) then
/// the `0xFF` terminator.
fn write_items<'a>(items: impl Iterator<Item = &'a DataItem>, p: &mut PacketWriter) {
    for it in items {
        p.write_u8(it.index);
        p.write_varint(it.value.serializer_id());
        it.value.encode(p);
    }
    p.write_u8(EOF_MARKER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;
    use bytes::Bytes;

    fn packed(data: &EntityData) -> PacketReader {
        let mut p = PacketWriter::new();
        data.write_packed(&mut p);
        PacketReader::new(Bytes::from(p.buf.to_vec()))
    }

    #[test]
    fn empty_data_is_just_the_terminator() {
        let data = EntityData::new();
        let mut p = PacketWriter::new();
        data.write_packed(&mut p);
        assert_eq!(&p.buf[..], &[EOF_MARKER]);
    }

    #[test]
    fn byte_and_pose_round_trip() {
        // Mirrors the player shared-flags (index 0, BYTE) + pose (index 6, POSE).
        let mut data = EntityData::new();
        data.set(0, DataValue::Byte(0x0A))
            .set(6, DataValue::Pose(5));
        let mut r = packed(&data);
        assert_eq!(r.read_u8().unwrap(), 0); // accessor index 0
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_BYTE);
        assert_eq!(r.read_u8().unwrap(), 0x0A);
        assert_eq!(r.read_u8().unwrap(), 6); // accessor index 6
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_POSE);
        assert_eq!(r.read_varint().unwrap(), 5);
        assert_eq!(r.read_u8().unwrap(), EOF_MARKER);
    }

    #[test]
    fn item_stack_entry_layout() {
        // ItemEntity DATA_ITEM: accessor index 8, ITEM_STACK serializer (7).
        let mut data = EntityData::new();
        data.set(8, DataValue::ItemStack(Some(ItemStack::new(724, 3))));
        let mut r = packed(&data);
        assert_eq!(r.read_u8().unwrap(), 8);
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_ITEM_STACK);
        // OPTIONAL_STREAM_CODEC: count, id, then empty component patch (0, 0).
        assert_eq!(r.read_varint().unwrap(), 3); // count
        assert_eq!(r.read_varint().unwrap(), 724); // item id
        assert_eq!(r.read_varint().unwrap(), 0); // added components
        assert_eq!(r.read_varint().unwrap(), 0); // removed components
        assert_eq!(r.read_u8().unwrap(), EOF_MARKER);
    }

    #[test]
    fn int_entry_layout() {
        // ExperienceOrb DATA_VALUE: accessor index 8, INT serializer (1).
        let mut data = EntityData::new();
        data.set(8, DataValue::Int(7));
        let mut r = packed(&data);
        assert_eq!(r.read_u8().unwrap(), 8);
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_INT);
        assert_eq!(r.read_varint().unwrap(), 7);
        assert_eq!(r.read_u8().unwrap(), EOF_MARKER);
    }

    #[test]
    fn optional_block_pos_present_and_absent() {
        let mut present = EntityData::new();
        present.set(3, DataValue::OptionalBlockPos(Some((1, 64, -3))));
        let mut r = packed(&present);
        assert_eq!(r.read_u8().unwrap(), 3);
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_OPTIONAL_BLOCK_POS);
        assert!(r.read_bool().unwrap()); // present flag
        assert_eq!(r.read_block_pos().unwrap(), (1, 64, -3));
        assert_eq!(r.read_u8().unwrap(), EOF_MARKER);

        let mut absent = EntityData::new();
        absent.set(3, DataValue::OptionalBlockPos(None));
        let mut r = packed(&absent);
        assert_eq!(r.read_u8().unwrap(), 3);
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_OPTIONAL_BLOCK_POS);
        assert!(!r.read_bool().unwrap()); // absent flag, no pos follows
        assert_eq!(r.read_u8().unwrap(), EOF_MARKER);
    }

    #[test]
    fn set_replaces_existing_index() {
        let mut data = EntityData::new();
        data.set(8, DataValue::Int(1)).set(8, DataValue::Int(2));
        assert_eq!(data.items().len(), 1);
        assert_eq!(data.items()[0].value, DataValue::Int(2));
    }

    #[test]
    fn pack_dirty_emits_then_clears() {
        let mut data = EntityData::new();
        data.set(0, DataValue::Byte(0x0A))
            .set(8, DataValue::Int(7));

        // A freshly-set binding is dirty and packs once.
        let dirty = data.pack_dirty();
        assert_eq!(dirty.len(), 2);
        assert_eq!(dirty[0].index, 0);
        assert_eq!(dirty[0].value, DataValue::Byte(0x0A));
        assert_eq!(dirty[1].index, 8);
        assert_eq!(dirty[1].value, DataValue::Int(7));

        // Flags cleared: a second flush with no further changes emits nothing.
        assert!(data.pack_dirty().is_empty());

        // Re-setting the same value stays clean; a real change re-dirties only it.
        data.set(8, DataValue::Int(7));
        assert!(data.pack_dirty().is_empty());
        data.set(8, DataValue::Int(9));
        let dirty = data.pack_dirty();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].index, 8);
        assert_eq!(dirty[0].value, DataValue::Int(9));
        assert!(data.pack_dirty().is_empty());
    }

    #[test]
    fn write_dirty_reports_and_clears() {
        let mut data = EntityData::new();
        data.set(6, DataValue::Pose(5));

        let mut p = PacketWriter::new();
        assert!(data.write_dirty(&mut p)); // emitted the one dirty entry
        let mut r = PacketReader::new(Bytes::from(p.buf.to_vec()));
        assert_eq!(r.read_u8().unwrap(), 6);
        assert_eq!(r.read_varint().unwrap(), SERIALIZER_POSE);
        assert_eq!(r.read_varint().unwrap(), 5);
        assert_eq!(r.read_u8().unwrap(), EOF_MARKER);

        // Nothing left dirty: still writes the terminator, but reports false.
        let mut p2 = PacketWriter::new();
        assert!(!data.write_dirty(&mut p2));
        assert_eq!(&p2.buf[..], &[EOF_MARKER]);
    }

    #[test]
    fn float_and_boolean_serializer_ids() {
        assert_eq!(DataValue::Float(1.0).serializer_id(), SERIALIZER_FLOAT);
        assert_eq!(DataValue::Boolean(true).serializer_id(), SERIALIZER_BOOLEAN);
    }
}
