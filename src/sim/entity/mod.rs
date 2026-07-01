//! Non-player entities: the ECS components, the spawn API, and the tracking that
//! makes them appear on and disappear from nearby clients.
//!
//! Players are still their own thing (they carry `Profile`/`Tracking` and a
//! connection). Everything else in the world — dropped items, XP orbs, and the
//! mobs/projectiles that will follow — is a *net entity*: a numeric id, a UUID, a
//! type, a position, and a [`syncher::EntityData`] metadata set. This module owns
//! that model and the wire path to viewers, generalizing the previously
//! player-only spawn/track code.
//!
//! Tracking mirrors vanilla's `ServerEntity` pairing for the spawn case: a viewer
//! is sent `ClientboundAddEntityPacket` followed by `ClientboundSetEntityDataPacket`
//! (the non-default metadata) — see `ServerEntity.sendPairingData`. Movement of
//! these entities is not modelled yet (they spawn at rest); when it is, they gain
//! a `Tracking` component and join the `broadcast_movement` path.
//!
//! Per-viewer culling reuses the player's [`LoadedChunks`] set: an entity is sent
//! only to players that have its column loaded, matching how vanilla scopes an
//! entity tracker to players whose chunk view covers it.

pub mod packets;
pub mod syncher;

use bevy_ecs::prelude::*;
use rand::RngCore;
use uuid::Uuid;

use super::bridge::{Outbound, OutboxTx};
use super::components::*;
use crate::inventory::ItemStack;
use syncher::{DataValue, EntityData};

// --- Entity-type registry ids (see `registry::builtin::ENTITY_TYPE`) ----------
// Hard-coded (like the player's id in `sim::packets`) with a test below pinning
// them to the registry so a reordering is caught.
//
// The item path is now live: breaking a block in survival spawns a
// `minecraft:item` via `spawn_item_entity` (see `sim::packet_handlers`). The XP
// orb path still has no in-game trigger (mob/ore XP lands later), so its spawn
// helper and constant read as dead in a non-test build.
/// `minecraft:item` — the dropped-item entity type.
const ENTITY_TYPE_ITEM: i32 = 71;
/// `minecraft:experience_orb`.
#[allow(dead_code)]
const ENTITY_TYPE_EXPERIENCE_ORB: i32 = 49;

// --- Metadata accessor indices ------------------------------------------------
// `SynchedEntityData.defineId` assigns indices in class-hierarchy order; `Entity`
// itself occupies 0..=7, so the first field of a direct `Entity` subclass is 8.
/// `ItemEntity.DATA_ITEM` — the carried `ItemStack` (accessor index 8).
const ITEM_ENTITY_DATA_ITEM: u8 = 8;
/// `ExperienceOrb.DATA_VALUE` — the orb's XP amount (accessor index 8).
#[allow(dead_code)]
const EXPERIENCE_ORB_DATA_VALUE: u8 = 8;

/// A world entity that is not a player: the identity the client needs to spawn
/// and address it. Players carry `Profile` instead (their numeric id lives there).
#[derive(Component)]
pub struct NetEntity {
    /// Network entity id (shared id space with players; from [`NextEntityId`]).
    pub id: i32,
    /// Stable UUID sent in `AddEntity`.
    pub uuid: Uuid,
    /// `entity_type` registry id.
    pub type_id: i32,
}

/// The entity's synchronized metadata (`SynchedEntityData`), sent after its
/// `AddEntity` and resent on change.
#[derive(Component)]
pub struct EntityMeta(pub EntityData);

/// Marker for dropped-item entities (`minecraft:item`). Inserted by
/// [`spawn_item_entity`] on every survival block break; the item-pickup system
/// (another module) queries `(&NetEntity, &Pos, &ItemDrop, &EntityMeta)` to find
/// stacks to hand to a nearby player.
#[derive(Component)]
pub struct ItemDrop;

/// Marker for XP orbs (`minecraft:experience_orb`).
#[derive(Component)]
#[allow(dead_code)] // classifies spawned orbs; queried once gameplay (pickup) exists.
pub struct XpOrb;

/// The chunk column `(cx, cz)` a world position sits in
/// (`floor(x)>>4, floor(z)>>4`), the key used for per-viewer culling.
fn chunk_of(x: f64, z: f64) -> (i32, i32) {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// Allocate the next network entity id (shared with players).
fn alloc_id(world: &mut World) -> i32 {
    let mut next = world.resource_mut::<NextEntityId>();
    let v = next.0;
    next.0 += 1;
    v
}

/// A fresh random entity UUID. Non-player entities have no account identity, so
/// any unique value works (vanilla uses `Mth.createInsecureUUID`).
fn random_uuid() -> Uuid {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    Uuid::from_bytes(bytes)
}

/// Spawn a dropped-item entity carrying `stack` at `(x, y, z)` and show it to
/// every player tracking that column. Returns the network entity id.
///
/// The metadata mirrors `ItemEntity`: `DATA_ITEM` (index 8) holds the stack.
/// Pickup and physics are not modelled yet — the item renders and sits where it
/// spawned.
///
/// Live trigger: `sim::packet_handlers` calls this for each stack returned by
/// `world::block_drop::drops_for` when a survival player finishes breaking a block.
pub fn spawn_item_entity(world: &mut World, pos: (f64, f64, f64), stack: ItemStack) -> i32 {
    let id = alloc_id(world);
    let uuid = random_uuid();
    let mut meta = EntityData::new();
    meta.set(ITEM_ENTITY_DATA_ITEM, DataValue::ItemStack(Some(stack)));

    spawn_tracked(
        world,
        NetEntity { id, uuid, type_id: ENTITY_TYPE_ITEM },
        pos,
        meta,
        |ec| {
            ec.insert(ItemDrop);
        },
    );
    id
}

/// Spawn an XP orb worth `value` at `(x, y, z)` and show it to every player
/// tracking that column. Returns the network entity id.
///
/// In 26.2 the orb's value is not carried in `AddEntity`'s `data` field but in
/// its `DATA_VALUE` metadata (index 8), so the spawn is a generic `AddEntity`
/// (data 0) followed by the metadata — same as any other entity.
#[allow(dead_code)] // public spawn API; no in-game trigger (mob/xp drop) yet.
pub fn spawn_xp_orb(world: &mut World, pos: (f64, f64, f64), value: i32) -> i32 {
    let id = alloc_id(world);
    let uuid = random_uuid();
    let mut meta = EntityData::new();
    meta.set(EXPERIENCE_ORB_DATA_VALUE, DataValue::Int(value));

    spawn_tracked(
        world,
        NetEntity { id, uuid, type_id: ENTITY_TYPE_EXPERIENCE_ORB },
        pos,
        meta,
        |ec| {
            ec.insert(XpOrb);
        },
    );
    id
}

/// Spawn a net entity into the world and pair it to every tracking viewer.
/// `tag` inserts any type-specific marker component onto the new entity.
fn spawn_tracked(
    world: &mut World,
    net: NetEntity,
    pos: (f64, f64, f64),
    meta: EntityData,
    tag: impl FnOnce(&mut bevy_ecs::world::EntityWorldMut),
) {
    let (x, y, z) = pos;
    let (id, uuid, type_id) = (net.id, net.uuid, net.type_id);

    // Build the pairing packets once, then fan out to viewers.
    let add = packets::add_entity(id, uuid, type_id, pos, 0, 0, 0, 0);
    let data = packets::set_entity_data(id, &meta);

    let mut ec = world.spawn((
        net,
        Pos { x, y, z, yaw: 0.0, pitch: 0.0, on_ground: false },
        EntityMeta(meta),
    ));
    tag(&mut ec);

    send_to_viewers(world, chunk_of(x, z), &[add, data]);
}

/// Send framed packets to every player whose loaded-chunk set covers `chunk`.
/// Best-effort per the outbox contract (a momentarily-full outbox drops the
/// send); this is the same delivery guarantee the player spawn path uses.
fn send_to_viewers(world: &mut World, chunk: (i32, i32), pkts: &[bytes::Bytes]) {
    let mut q = world.query::<(&LoadedChunks, &Conn)>();
    for (loaded, conn) in q.iter(world) {
        if loaded.loaded.contains(&chunk) {
            for pkt in pkts {
                let _ = conn.outbox.try_send(Outbound::Packet(pkt.clone()));
            }
        }
    }
}

/// Replay every existing net entity to a newcomer whose loaded-chunk set is
/// `loaded`. Called from the join path after the newcomer's chunks are seeded, so
/// items and orbs already in the world render for the arriving player — the
/// non-player counterpart to spawning existing players to a newcomer.
pub fn spawn_existing_entities_for(
    world: &mut World,
    outbox: &OutboxTx,
    loaded: &std::collections::HashSet<(i32, i32)>,
) {
    let mut q = world.query::<(&NetEntity, &Pos, &EntityMeta)>();
    for (net, pos, meta) in q.iter(world) {
        if !loaded.contains(&chunk_of(pos.x, pos.z)) {
            continue;
        }
        let add = packets::add_entity(
            net.id,
            net.uuid,
            net.type_id,
            (pos.x, pos.y, pos.z),
            0,
            0,
            0,
            0,
        );
        let data = packets::set_entity_data(net.id, &meta.0);
        let _ = outbox.try_send(Outbound::Packet(add));
        let _ = outbox.try_send(Outbound::Packet(data));
    }
}

/// Despawn a net entity and tell its viewers to remove it. Returns the removed
/// network id, or `None` if `entity` was not a net entity.
#[allow(dead_code)] // removal API for the gameplay layer (pickup/expiry) once it exists.
pub fn remove_entity(world: &mut World, entity: Entity) -> Option<i32> {
    let (id, chunk) = {
        let net = world.get::<NetEntity>(entity)?;
        let pos = world.get::<Pos>(entity)?;
        (net.id, chunk_of(pos.x, pos.z))
    };
    world.despawn(entity);
    let remove = super::packets::remove_entities(&[id]);
    send_to_viewers(world, chunk, std::slice::from_ref(&remove));
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;
    use crate::sim::bridge::Outbound;
    use std::collections::HashSet;
    use tokio::sync::mpsc;

    fn frame_id(b: &bytes::Bytes) -> i32 {
        let mut r = PacketReader::new(b.clone());
        r.read_varint().unwrap(); // length
        r.read_varint().unwrap() // id
    }

    fn drain(rx: &mut mpsc::Receiver<Outbound>) -> Vec<i32> {
        let mut ids = Vec::new();
        while let Ok(Outbound::Packet(b)) = rx.try_recv() {
            ids.push(frame_id(&b));
        }
        ids
    }

    /// A minimal viewer whose loaded set covers `chunks`.
    fn spawn_viewer(world: &mut World, chunks: &[(i32, i32)]) -> mpsc::Receiver<Outbound> {
        let (tx, rx) = mpsc::channel(64);
        world.spawn((
            Conn { outbox: tx },
            LoadedChunks {
                center: (0, 0),
                loaded: chunks.iter().copied().collect::<HashSet<_>>(),
            },
        ));
        rx
    }

    fn expected_ids() -> (i32, i32) {
        (
            frame_id(&super::packets::add_entity(
                0,
                Uuid::nil(),
                0,
                (0.0, 0.0, 0.0),
                0,
                0,
                0,
                0,
            )),
            frame_id(&super::packets::set_entity_data(0, &EntityData::new())),
        )
    }

    fn world_with_id() -> World {
        let mut world = World::new();
        world.insert_resource(NextEntityId(1));
        world
    }

    #[test]
    fn item_spawn_pairs_add_then_data_to_in_range_viewer() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        let id = spawn_item_entity(&mut world, (1.0, 64.0, 2.0), ItemStack::new(1, 1));
        assert_eq!(id, 1);
        let (add_id, data_id) = expected_ids();
        assert_eq!(drain(&mut rx), vec![add_id, data_id]);
    }

    #[test]
    fn out_of_range_viewer_receives_nothing() {
        let mut world = world_with_id();
        // Viewer only tracks chunk (10,10); the item spawns at chunk (0,0).
        let mut rx = spawn_viewer(&mut world, &[(10, 10)]);
        spawn_xp_orb(&mut world, (5.0, 64.0, 5.0), 7);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn join_replay_sends_existing_entities_in_range() {
        let mut world = world_with_id();
        // No viewers yet, so the spawn broadcasts to nobody.
        spawn_item_entity(&mut world, (1.0, 64.0, 2.0), ItemStack::new(1, 1));
        spawn_xp_orb(&mut world, (300.0, 64.0, 0.0), 3); // chunk (18,0), out of range

        let (tx, mut rx) = mpsc::channel(64);
        let loaded: HashSet<(i32, i32)> = [(0, 0)].into_iter().collect();
        spawn_existing_entities_for(&mut world, &tx, &loaded);

        // Only the in-range item is replayed: AddEntity + SetEntityData.
        let (add_id, data_id) = expected_ids();
        assert_eq!(drain(&mut rx), vec![add_id, data_id]);
    }

    #[test]
    fn remove_entity_broadcasts_removal_and_despawns() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        let id = spawn_item_entity(&mut world, (1.0, 64.0, 2.0), ItemStack::new(1, 1));
        let _ = drain(&mut rx); // discard the spawn pair

        // Find the ECS entity for the spawned net id and remove it.
        let ecs = world
            .query::<(Entity, &NetEntity)>()
            .iter(&world)
            .find(|(_, n)| n.id == id)
            .map(|(e, _)| e)
            .unwrap();
        let removed = remove_entity(&mut world, ecs);
        assert_eq!(removed, Some(id));

        let remove_id = frame_id(&super::super::packets::remove_entities(&[0]));
        assert_eq!(drain(&mut rx), vec![remove_id]);
        assert_eq!(world.query::<&NetEntity>().iter(&world).count(), 0);
    }

    #[test]
    fn entity_type_ids_match_registry() {
        use crate::registry::builtin::ENTITY_TYPE;
        assert_eq!(ENTITY_TYPE.id_of("minecraft:item"), Some(ENTITY_TYPE_ITEM));
        assert_eq!(
            ENTITY_TYPE.id_of("minecraft:experience_orb"),
            Some(ENTITY_TYPE_EXPERIENCE_ORB)
        );
    }
}
