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
/// `ClientboundDamageEventPacket` — index 25 in the clientbound PLAY flow.
const CB_PLAY_DAMAGE_EVENT: i32 = 25;
/// `ClientboundEntityEventPacket` — index 34 in the clientbound PLAY flow.
const CB_PLAY_ENTITY_EVENT: i32 = 34;
/// `ClientboundSetEntityDataPacket` — index 99 in the clientbound PLAY flow.
const CB_PLAY_SET_ENTITY_DATA: i32 = 99;
/// `ClientboundHurtAnimationPacket` — index 42 in the clientbound PLAY flow.
#[allow(dead_code)] // superseded by `damage_event` on the mob path; kept for the player-hurt seam.
const CB_PLAY_HURT_ANIMATION: i32 = 42;
/// `ClientboundSoundPacket` — index 117 in the clientbound PLAY flow.
const CB_PLAY_SOUND: i32 = 117;

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
#[allow(dead_code)] // superseded by `damage_event`; retained for the player-hurt seam.
pub fn hurt_animation(entity_id: i32, yaw: f32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_f32(yaw);
    frame(CB_PLAY_HURT_ANIMATION, &p.buf)
}

/// `ClientboundDamageEventPacket` — what `LivingEntity.hurtServer` broadcasts on
/// a full hit (`ServerLevel.broadcastDamageEvent`). The client derives the red
/// hurt flash *and* the attacker-relative hurt-direction lean from it (this is
/// what supersedes [`hurt_animation`] for damaged entities).
///
/// Layout (`ClientboundDamageEventPacket.write`): entity id (VarInt); the
/// `Holder<DamageType>` source type as `ByteBufCodecs.holderRegistry` — a VarInt
/// of the damage-type registry id **+1** (0 is reserved for an inline/direct
/// holder, which registry damage types never are); the cause entity id and the
/// direct entity id, each written as `id + 1` (so a missing entity, id `-1`,
/// encodes as `0`); then an optional source position (a `bool` present-flag and,
/// when present, three f64). Melee carries no source position → the flag is false.
pub fn damage_event(
    entity_id: i32,
    damage_type_id: i32,
    cause_id: i32,
    direct_id: i32,
    source_pos: Option<(f64, f64, f64)>,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(entity_id);
    p.write_varint(damage_type_id + 1); // holderRegistry: registry id + 1
    p.write_varint(cause_id + 1); // writeOptionalEntityId: id + 1 (−1 → 0)
    p.write_varint(direct_id + 1);
    match source_pos {
        Some((x, y, z)) => {
            p.write_bool(true);
            p.write_f64(x);
            p.write_f64(y);
            p.write_f64(z);
        }
        None => p.write_bool(false),
    }
    frame(CB_PLAY_DAMAGE_EVENT, &p.buf)
}

/// `ClientboundEntityEventPacket` — a one-byte entity event broadcast to
/// trackers (`Level.broadcastEntityEvent`). Layout: entity id as a **fixed** i32
/// (`writeInt`, not a VarInt) then the event byte. Vela uses event `60`
/// (`LivingEntity.tickDeath`'s `makePoofParticles`) when a corpse finishes its
/// death animation and is removed.
pub fn entity_event(entity_id: i32, event_id: u8) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_i32(entity_id);
    p.write_u8(event_id);
    frame(CB_PLAY_ENTITY_EVENT, &p.buf)
}

/// `ClientboundSoundPacket` — a positioned sound effect (`ServerLevel.playSound`,
/// the path `Entity.playSound`/`LivingEntity.makeSound` take). Layout
/// (`ClientboundSoundPacket.write`): the `Holder<SoundEvent>` as
/// `ByteBufCodecs.holder` — a VarInt of the `sound_event` registry id **+1** (0
/// is the inline form, unused here); the `SoundSource` category as its enum
/// ordinal (VarInt); the position as three **fixed** i32 of `floor(coord * 8)`
/// (`LOCATION_ACCURACY`); the volume and pitch (f32); and a random i64 seed.
#[allow(clippy::too_many_arguments)]
pub fn play_sound(
    sound_id: i32,
    source_ordinal: i32,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(sound_id + 1); // holder: registry id + 1
    p.write_varint(source_ordinal); // SoundSource ordinal (writeEnum)
    p.write_i32((x * 8.0) as i32); // (int)(x * 8.0)
    p.write_i32((y * 8.0) as i32);
    p.write_i32((z * 8.0) as i32);
    p.write_f32(volume);
    p.write_f32(pitch);
    p.write_i64(seed);
    frame(CB_PLAY_SOUND, &p.buf)
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

    /// Read a fixed big-endian i32 (`PacketReader` has no `read_i32`).
    fn read_i32(r: &mut PacketReader) -> i32 {
        let b = r.read_bytes(4).unwrap();
        i32::from_be_bytes([b[0], b[1], b[2], b[3]])
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
    fn damage_event_layout_no_position() {
        // entity 9, damage type 33 (player_attack), attacker 4, no source position.
        let (id, mut r) = unframe(damage_event(9, 33, 4, 4, None));
        assert_eq!(id, CB_PLAY_DAMAGE_EVENT);
        assert_eq!(r.read_varint().unwrap(), 9); // entity id
        assert_eq!(r.read_varint().unwrap(), 34); // damage type id + 1
        assert_eq!(r.read_varint().unwrap(), 5); // cause id + 1
        assert_eq!(r.read_varint().unwrap(), 5); // direct id + 1
        assert!(!r.read_bool().unwrap()); // no source position
        assert!(r.read_u8().is_err(), "no trailing bytes");
    }

    #[test]
    fn damage_event_missing_entity_encodes_zero() {
        // A `-1` (no) cause/direct entity encodes as 0.
        let (_id, mut r) = unframe(damage_event(9, 18, -1, -1, Some((1.0, 2.0, 3.0))));
        assert_eq!(r.read_varint().unwrap(), 9);
        assert_eq!(r.read_varint().unwrap(), 19); // damage type id + 1
        assert_eq!(r.read_varint().unwrap(), 0); // cause -1 → 0
        assert_eq!(r.read_varint().unwrap(), 0); // direct -1 → 0
        assert!(r.read_bool().unwrap()); // source position present
        assert_eq!(r.read_f64().unwrap(), 1.0);
        assert_eq!(r.read_f64().unwrap(), 2.0);
        assert_eq!(r.read_f64().unwrap(), 3.0);
    }

    #[test]
    fn entity_event_layout() {
        let (id, mut r) = unframe(entity_event(9, 60));
        assert_eq!(id, CB_PLAY_ENTITY_EVENT);
        assert_eq!(read_i32(&mut r), 9); // entity id is a FIXED int, not a VarInt
        assert_eq!(r.read_u8().unwrap(), 60); // poof-particles event
    }

    #[test]
    fn play_sound_layout() {
        // sound id 100, source NEUTRAL (6), position quantized ×8.
        let (id, mut r) = unframe(play_sound(100, 6, 1.5, 64.0, -2.25, 1.0, 1.2, 42));
        assert_eq!(id, CB_PLAY_SOUND);
        assert_eq!(r.read_varint().unwrap(), 101); // sound id + 1
        assert_eq!(r.read_varint().unwrap(), 6); // SoundSource.NEUTRAL ordinal
        assert_eq!(read_i32(&mut r), 12); // floor(1.5 * 8)
        assert_eq!(read_i32(&mut r), 512); // 64 * 8
        assert_eq!(read_i32(&mut r), -18); // (int)(-2.25 * 8) = -18
        assert_eq!(r.read_f32().unwrap(), 1.0); // volume
        assert_eq!(r.read_f32().unwrap(), 1.2); // pitch
        assert_eq!(r.read_i64().unwrap(), 42); // seed
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
