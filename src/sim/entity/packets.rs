//! Clientbound packet builders for the generic-entity domain.
//!
//! Non-player entities (items, XP orbs, and everything that follows) share a
//! spawn/metadata path distinct from the player-specific builders in
//! `sim::packets`, so — like `crate::inventory` — this domain keeps its own
//! builders and packet ids next to its data model. The ids are the same
//! registration-order indices as the shared module (re-declared here so the
//! domain is self-contained).

use bytes::Bytes;
use uuid::Uuid;

use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;

use super::syncher::EntityData;

/// `ClientboundAddEntityPacket` — index 1 in the clientbound PLAY flow
/// (`GameProtocols.CLIENTBOUND_TEMPLATE`, past the bundle delimiter at 0).
const CB_PLAY_ADD_ENTITY: i32 = 1;
/// `ClientboundSetEntityDataPacket` — index 99 in the clientbound PLAY flow.
const CB_PLAY_SET_ENTITY_DATA: i32 = 99;
/// `ClientboundHurtAnimationPacket` — index 42 in the clientbound PLAY flow.
const CB_PLAY_HURT_ANIMATION: i32 = 42;

/// `ClientboundAddEntityPacket` for an arbitrary entity type — the generic spawn
/// path (the player builder in `sim::packets` is a fixed-type specialization).
///
/// Layout (`ClientboundAddEntityPacket.write`): entity id (VarInt), UUID, entity
/// type (VarInt registry id over `Registries.ENTITY_TYPE`), the position
/// (three f64), the `LpVec3` movement, then packed `xRot`/`yRot`/`yHeadRot`
/// bytes, then the `data` VarInt. Angles are already packed (`pack_angle`).
///
/// Movement is written as a zero `LpVec3` (a single `0` byte) — Vela doesn't yet
/// model velocity for spawned entities, so they arrive at rest and the server
/// drives any later motion via move packets. `data` is the type-specific spawn
/// datum (0 for items and XP orbs in 26.2, where the orb's value travels as
/// metadata rather than in this field).
#[allow(clippy::too_many_arguments)]
pub fn add_entity(
    entity_id: i32,
    uuid: Uuid,
    type_id: i32,
    pos: (f64, f64, f64),
    yaw: i8,
    pitch: i8,
    head: i8,
    data: i32,
) -> Bytes {
    let (x, y, z) = pos;
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_uuid(uuid);
    p.write_varint(type_id);
    p.write_f64(x);
    p.write_f64(y);
    p.write_f64(z);
    p.write_u8(0); // movement: zero LpVec3 is a single 0 byte
    p.write_u8(pitch as u8); // xRot
    p.write_u8(yaw as u8); // yRot
    p.write_u8(head as u8); // yHeadRot
    p.write_varint(data);
    frame(CB_PLAY_ADD_ENTITY, &p.buf)
}

/// `ClientboundSetEntityDataPacket` for a full [`EntityData`] set. Layout:
/// `entity_id` (VarInt) then the packed metadata list (see
/// [`EntityData::write_packed`]) terminated by `0xFF`.
pub fn set_entity_data(entity_id: i32, data: &EntityData) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    data.write_packed(&mut p);
    frame(CB_PLAY_SET_ENTITY_DATA, &p.buf)
}

/// `ClientboundHurtAnimationPacket` — plays the red damage flash (and a small
/// directional lean) on `entity_id` for tracking clients. Layout: entity id
/// (VarInt) then the hurt-direction yaw (f32). We pass yaw 0.0 — the attacker-
/// relative lean needs the knockback/direction model Vela lacks, but the flash,
/// the part players read as "it got hit", plays regardless.
pub fn hurt_animation(entity_id: i32, yaw: f32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_f32(yaw);
    frame(CB_PLAY_HURT_ANIMATION, &p.buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::ItemStack;
    use crate::protocol::buffer::PacketReader;
    use super::super::syncher::DataValue;

    fn unframe(bytes: Bytes) -> (i32, PacketReader) {
        let mut r = PacketReader::new(bytes);
        let _len = r.read_varint().unwrap();
        let id = r.read_varint().unwrap();
        (id, r)
    }

    #[test]
    fn add_entity_generic_layout() {
        let uuid = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        // 72 is an arbitrary entity type id for this layout test; the real
        // dropped-item type is ENTITY_TYPE_ITEM = 71 in the 26.2 registry.
        let (id, mut r) = unframe(add_entity(9, uuid, 72, (1.0, 64.0, -2.0), 0, 0, 0, 0));
        assert_eq!(id, CB_PLAY_ADD_ENTITY);
        assert_eq!(r.read_varint().unwrap(), 9); // entity id
        assert_eq!(r.read_uuid().unwrap(), uuid);
        assert_eq!(r.read_varint().unwrap(), 72); // entity type
        assert_eq!(r.read_f64().unwrap(), 1.0);
        assert_eq!(r.read_f64().unwrap(), 64.0);
        assert_eq!(r.read_f64().unwrap(), -2.0);
        assert_eq!(r.read_u8().unwrap(), 0); // zero LpVec3 movement
        assert_eq!(r.read_u8().unwrap(), 0); // xRot
        assert_eq!(r.read_u8().unwrap(), 0); // yRot
        assert_eq!(r.read_u8().unwrap(), 0); // yHeadRot
        assert_eq!(r.read_varint().unwrap(), 0); // data
    }

    #[test]
    fn hurt_animation_layout() {
        let (id, mut r) = unframe(hurt_animation(9, 0.0));
        assert_eq!(id, CB_PLAY_HURT_ANIMATION);
        assert_eq!(r.read_varint().unwrap(), 9); // entity id
        assert_eq!(r.read_f32().unwrap(), 0.0); // hurt-direction yaw
    }

    #[test]
    fn set_entity_data_carries_item_metadata() {
        let mut data = EntityData::new();
        data.set(8, DataValue::ItemStack(Some(ItemStack::new(1, 5))));
        let (id, mut r) = unframe(set_entity_data(9, &data));
        assert_eq!(id, CB_PLAY_SET_ENTITY_DATA);
        assert_eq!(r.read_varint().unwrap(), 9); // entity id
        assert_eq!(r.read_u8().unwrap(), 8); // accessor index
        assert_eq!(r.read_varint().unwrap(), 7); // ITEM_STACK serializer
        assert_eq!(r.read_varint().unwrap(), 5); // stack count
        assert_eq!(r.read_varint().unwrap(), 1); // item id
        assert_eq!(r.read_varint().unwrap(), 0); // components added
        assert_eq!(r.read_varint().unwrap(), 0); // components removed
        assert_eq!(r.read_u8().unwrap(), 0xFF); // terminator
    }
}
